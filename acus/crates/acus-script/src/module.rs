// SPDX-License-Identifier: MPL-2.0
//! Runs and checks saved scripts: ES modules whose imports the host supplies.

use crate::engine::{Outcome, Sandbox, engine, execute, throw};
use crate::{Host, HostError, Limits, ScriptError};
use rquickjs::loader::{ImportAttributes, Loader, Resolver};
use rquickjs::module::Declared;
use rquickjs::{Ctx, Module, Value as JsValue};
use serde_json::Value;
use std::sync::Arc;

const PRELUDE: &str = include_str!("prelude.js");

/// What a saved script declares about itself.
#[derive(Debug)]
pub struct ModuleInfo {
    /// The `export const description` string.
    pub description: String,
    /// The `export const params` value as JSON.
    pub params: Value,
}

/// Accepts only specifiers with a scheme, so a script cannot reach files by path.
pub(crate) struct Imports;

impl Resolver for Imports {
    fn resolve<'js>(
        &mut self,
        ctx: &Ctx<'js>,
        _base: &str,
        name: &str,
        _attributes: Option<ImportAttributes<'js>>,
    ) -> rquickjs::Result<String> {
        let scheme = name.split_once(':').is_some_and(|(scheme, _)| {
            !scheme.is_empty() && scheme.chars().all(|c| c.is_ascii_lowercase())
        });
        if scheme {
            return Ok(name.to_owned());
        }
        Err(throw(
            ctx,
            &HostError::new(format!(
                "import \"{name}\" is refused: only scheme imports work"
            ))
            .with_hint("import a saved script as \"repo:NAME@V\" or \"user:NAME\""),
        ))
    }
}

/// Loads each imported module from the host.
pub(crate) struct HostModules<H>(pub(crate) Arc<H>);

impl<H: Host> Loader for HostModules<H> {
    fn load<'js>(
        &mut self,
        ctx: &Ctx<'js>,
        name: &str,
        _attributes: Option<ImportAttributes<'js>>,
    ) -> rquickjs::Result<Module<'js, Declared>> {
        let source = self.0.module(name).map_err(|e| throw(ctx, &e))?;
        Module::declare(ctx.clone(), name, source)
    }
}

/// Imports `entry`, calls its default export with `args` (JSON) and returns what it returns.
///
/// # Errors
/// Returns the failure of the program or of the engine; see [`ScriptError`].
pub fn run_module<H: Host>(
    entry: &str,
    args: &Value,
    host: H,
    limits: &Limits,
) -> Result<Outcome, ScriptError> {
    let names = serde_json::to_string(host.names())
        .map_err(|e| ScriptError::Engine(format!("cannot name the host functions: {e}")))?;
    let entry = Value::String(entry.to_owned());
    let source = format!(
        "{PRELUDE}\n__install({names});\n(async () => {{ const m = await import({entry}); \
         return await m.default({args}); }})().then(__finish)"
    );
    execute(&source, 0, host, limits)
}

/// Parses `source` as a module and evaluates it, so imports resolve and the exports exist.
///
/// A module that fails this check is not saved. Top-level code runs without the host's functions.
///
/// # Errors
/// Returns the module's own failure, or [`ScriptError::Invalid`] naming the missing export.
pub fn check_module<H: Host>(
    source: &str,
    host: H,
    limits: &Limits,
) -> Result<ModuleInfo, ScriptError> {
    let host = Arc::new(host);
    let sandbox = Sandbox::new(&host, limits)?;
    sandbox.context.with(|ctx| {
        let fail = |e| sandbox.fail(&ctx, e, 0, limits);
        let module = Module::declare(ctx.clone(), "saved", source).map_err(fail)?;
        let (module, done) = module.eval().map_err(fail)?;
        done.finish::<()>().map_err(fail)?;
        let missing = |what: &str| ScriptError::Invalid(format!("the script lacks {what}"));
        let description = module
            .get::<_, Option<String>>("description")
            .ok()
            .flatten()
            .ok_or_else(|| missing("`export const description` (a string)"))?;
        let params: JsValue = module.get("params").map_err(|e| engine(&e))?;
        if params.is_undefined() {
            return Err(missing("`export const params`"));
        }
        let default: JsValue = module.get("default").map_err(|e| engine(&e))?;
        if !default.is_function() {
            return Err(missing("`export default async function (args)`"));
        }
        let params = ctx
            .json_stringify(params)
            .ok()
            .flatten()
            .and_then(|s| s.to_string().ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .ok_or_else(|| ScriptError::Invalid("`params` must be JSON data".to_owned()))?;
        Ok(ModuleInfo {
            description,
            params,
        })
    })
}
