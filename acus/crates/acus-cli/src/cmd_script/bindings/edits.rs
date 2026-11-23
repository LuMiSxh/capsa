// SPDX-License-Identifier: MPL-2.0
//! The host functions that change files: `patch`, `write` and `edit`. Each checks its paths against
//! the `Edit(...)` rules, then applies its ops as one all-or-nothing patch.

use super::super::argv::Call;
use super::super::editing;
use super::super::shapes;
use super::Bindings;
use super::args::{internal, parse};
use super::host_error;
use crate::{Cmd, cmd_patch};
use acus_edit::Op;
use acus_script::HostError;
use serde_json::{Value, json};
use std::time::Instant;

impl Bindings {
    /// Applies a patch text, or the edits of `patch([{path, old, new, all?}])`, as one
    /// all-or-nothing patch.
    pub(super) fn patch(
        &self,
        mut args: Vec<Value>,
        deadline: Instant,
    ) -> Result<Value, HostError> {
        let items = match args.first() {
            Some(Value::Array(items)) if items.first().is_some_and(Value::is_object) => {
                Some(items.clone())
            }
            _ => None,
        };
        let (ops, mut call) = if let Some(items) = items {
            args.remove(0);
            let ops = editing::batch_ops(&items, &self.permissions, &self.cwd)?;
            (ops, Call::new("patch", args)?)
        } else {
            let mut call = Call::new("patch", args)?;
            let text = std::mem::take(&mut call.positionals).join("\n");
            (
                cmd_patch::parse_ops(&text).map_err(|e| host_error(&e))?,
                call,
            )
        };
        call.positionals.clear();
        let then = call.text("run").map(str::to_owned);
        let then = then.filter(|_| !call.flag("check"));
        if let Some(command) = &then {
            self.guard(command)?;
        }
        let mut value = self.apply(&ops, &call)?;
        if let Some(command) = then {
            value["run"] = self.run(Call::new("run", vec![json!(command)])?, deadline)?;
        }
        Ok(value)
    }

    /// Applies ops after every path they change passes the `Edit(...)` rules.
    fn apply(&self, ops: &[Op], call: &Call) -> Result<Value, HostError> {
        let Cmd::Patch(a) = parse("patch", call, &["run"])? else {
            return Err(internal("patch"));
        };
        editing::check(&self.permissions, &self.cwd, ops)?;
        let report = cmd_patch::compute_ops(&a, ops, &self.cfg).map_err(|e| host_error(&e))?;
        shapes::patch(&report)
    }

    /// Writes `text` to a file inside the repository, creating or replacing it.
    pub(super) fn write(&self, args: &[Value]) -> Result<Value, HostError> {
        let [Value::String(path), Value::String(text)] = args else {
            return Err(HostError::new("write() takes a path and the text")
                .with_hint("write('notes.txt', 'text') creates or replaces the file"));
        };
        let full = editing::resolve(&self.permissions, &self.cwd, path)?;
        let ops = editing::write_ops(path, text, full.exists());
        let report = self.apply(&ops, &Call::new("patch", Vec::new())?)?;
        editing::write_exact(&full, text)?;
        Ok(report)
    }

    /// Replaces `old` by `new` in a file inside the repository: once, which `old` must match
    /// exactly, or everywhere with `{all: true}`.
    pub(super) fn edit(&self, args: &[Value]) -> Result<Value, HostError> {
        let [
            Value::String(path),
            Value::String(old),
            Value::String(new),
            rest @ ..,
        ] = args
        else {
            return Err(
                HostError::new("edit() takes a path, the old text and the new text").with_hint(
                    "edit('a.rs', 'old', 'new') or edit('a.rs', 'old', 'new', {all: true})",
                ),
            );
        };
        let all = rest
            .first()
            .and_then(|options| options.get("all"))
            .and_then(Value::as_bool)
            == Some(true);
        editing::resolve(&self.permissions, &self.cwd, path)?;
        let ops = [editing::edit_op(path, old, new, all)?];
        self.apply(&ops, &Call::new("patch", Vec::new())?)
    }
}
