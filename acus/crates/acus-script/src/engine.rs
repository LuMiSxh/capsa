// SPDX-License-Identifier: MPL-2.0
//! Evaluates one program in a fresh QuickJS runtime and maps what goes wrong to [`ScriptError`].

use crate::module::{HostModules, Imports};
use crate::pool::Calls;
use crate::{Host, HostError, Limits, Partial, ScriptError};
use regex::Regex;
use rquickjs::{Context, Ctx, Error, Exception, Function, Module, Object, Promise, Runtime};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const PRELUDE: &str = include_str!("prelude.js");

/// Time a call already running gets to end after the deadline passes.
const EXPIRY_GRACE: Duration = Duration::from_secs(2);

/// What a finished program produced.
#[derive(Debug)]
pub struct Outcome {
    /// The values passed to `print`, in order.
    pub prints: Vec<Value>,
    /// The program's return value; `None` when it returned `undefined`.
    pub value: Option<Value>,
}

/// Runs `code` as the body of an async function, so `await` and `return` work at its top level.
///
/// The host moves into the engine, whose worker threads share it, so it is a value, not a borrow.
/// Every host function returns a promise; calls made before the program awaits run concurrently
/// as far as the host's [`Host::exclusive`] allows.
///
/// An array in a printed or returned value that carries a `kind` comes back as
/// `{"kind": K, "items": [...]}`.
///
/// # Errors
/// Returns the failure of the program or of the engine; see [`ScriptError`].
pub fn run<H: Host>(code: &str, host: H, limits: &Limits) -> Result<Outcome, ScriptError> {
    let names = serde_json::to_string(host.names())
        .map_err(|e| ScriptError::Engine(format!("cannot name the host functions: {e}")))?;
    // The lines before the program: a stack line past them is the program's own.
    let head = format!("{PRELUDE}\n__install({names});\n(async () => {{\n");
    let offset = head.matches('\n').count();
    let source = format!(
        "{head}{}\n}})().then(__finish)",
        crate::source::implicit_return(&crate::imports::dynamic(code))
    );

    execute(&source, offset, host, limits)
}

/// Checks that `code` parses as an async function body, without running it or calling the host.
///
/// A saved script uses `check_module`, which also evaluates the module's top level so its exports
/// exist. This check only proves the syntax, so it catches a typo before a real run.
///
/// # Errors
/// Returns the syntax error with the program's own line when it is known.
pub fn check<H: Host>(code: &str, host: H, limits: &Limits) -> Result<(), ScriptError> {
    let names = serde_json::to_string(host.names())
        .map_err(|e| ScriptError::Engine(format!("cannot name the host functions: {e}")))?;
    let head = format!("{PRELUDE}\n__install({names});\n(async () => {{\n");
    let offset = head.matches('\n').count();
    let source = format!(
        "{head}{}\n}})().then(__finish)",
        crate::source::implicit_return(&crate::imports::dynamic(code))
    );
    let host = Arc::new(host);
    let sandbox = Sandbox::new(&host, limits)?;
    sandbox.context.with(|ctx| {
        Module::declare(ctx.clone(), "inline", source)
            .map(|_| ())
            .map_err(|e| sandbox.fail(&ctx, e, offset, limits))
    })
}

/// Evaluates `source`, a script that ends in `.then(__finish)`, and reads the document it built.
pub(crate) fn execute<H: Host>(
    source: &str,
    offset: usize,
    host: H,
    limits: &Limits,
) -> Result<Outcome, ScriptError> {
    let host = Arc::new(host);
    let sandbox = Sandbox::new(&host, limits)?;
    let calls = Rc::new(RefCell::new(Calls::new(
        Arc::clone(&host),
        sandbox.deadline,
    )));
    let result = sandbox
        .context
        .with(|ctx| {
            let submit = Rc::clone(&calls);
            let start = Function::new(ctx.clone(), move |id: u32, name: String, args: String| {
                submit.borrow_mut().submit(id, name, args);
            })
            .map_err(|e| engine(&e))?;
            ctx.globals().set("__host", start).map_err(|e| engine(&e))?;
            let mut finished = ctx
                .eval::<Promise, _>(source)
                .and_then(|promise| drive(&ctx, &promise, &calls, &sandbox));
            if !sandbox.expired() {
                // A call the program never awaited still runs to its end, as a patch must.
                sandbox.drain(&calls);
                if sandbox.expired() {
                    finished = Err(Error::WouldBlock);
                }
            }
            // A result read after the deadline, by a late thread, never lets the program succeed.
            if finished.is_ok() && Instant::now() > sandbox.deadline {
                sandbox.expire(&calls);
                finished = Err(Error::WouldBlock);
            }
            finished.map_err(|e| sandbox.fail(&ctx, e, offset, limits))
        })
        .map_err(|error| with_output(&sandbox, &calls, error))?;
    // The document the prelude's `__finish` builds: `prints`, `defined` and `result`.
    let mut done: Value = serde_json::from_str(&result)
        .map_err(|e| ScriptError::Engine(format!("the program's result is not JSON: {e}")))?;
    let prints = match done["prints"].take() {
        Value::Array(prints) => prints,
        _ => Vec::new(),
    };
    let defined = done["defined"].as_bool().unwrap_or(false);
    Ok(Outcome {
        prints,
        value: defined.then(|| done["result"].take()),
    })
}

/// Runs queued jobs and hands each finished host call to the prelude until `promise` settles.
fn drive<'js, H: Host>(
    ctx: &Ctx<'js>,
    promise: &Promise<'js>,
    calls: &RefCell<Calls<H>>,
    sandbox: &Sandbox,
) -> rquickjs::Result<String> {
    let settle: Function = ctx.globals().get("__settle")?;
    loop {
        while ctx.execute_pending_job() {}
        if let Some(result) = promise.result::<String>() {
            return result;
        }
        // The borrow ends before the prelude runs, since the program submits calls from there.
        if calls.borrow().idle() {
            return Err(Error::WouldBlock);
        }
        let Some(done) = calls.borrow_mut().wait(sandbox.deadline) else {
            sandbox.expire(calls);
            return Err(Error::WouldBlock);
        };
        let (ok, text) = match done.result {
            Ok(text) => (true, text),
            Err(e) => (
                false,
                json!({"message": e.message, "hint": e.hint}).to_string(),
            ),
        };
        settle.call::<_, ()>((done.id, ok, text))?;
    }
}

/// A QuickJS runtime and context bounded by [`Limits`], with modules loaded through the host.
pub(crate) struct Sandbox {
    runtime: Runtime,
    pub(crate) context: Context,
    expired: Arc<AtomicBool>,
    deadline: Instant,
}

impl Sandbox {
    pub(crate) fn new<H: Host>(host: &Arc<H>, limits: &Limits) -> Result<Self, ScriptError> {
        let runtime = Runtime::new().map_err(|e| engine(&e))?;
        runtime.set_memory_limit(limits.memory);
        runtime.set_max_stack_size(limits.stack);
        runtime.set_loader(Imports, HostModules(Arc::clone(host)));
        let expired = Arc::new(AtomicBool::new(false));
        let deadline = Instant::now() + limits.deadline;
        let flag = Arc::clone(&expired);
        runtime.set_interrupt_handler(Some(Box::new(move || {
            let late = Instant::now() > deadline;
            if late {
                flag.store(true, Ordering::Relaxed);
            }
            late
        })));
        let context = Context::full(&runtime).map_err(|e| engine(&e))?;
        Ok(Sandbox {
            runtime,
            context,
            expired,
            deadline,
        })
    }

    fn expired(&self) -> bool {
        self.expired.load(Ordering::Relaxed)
    }

    /// Waits for every queued and running call to end, or for the deadline.
    fn drain<H: Host>(&self, calls: &RefCell<Calls<H>>) {
        while !calls.borrow().idle() {
            if !absorb(calls, self.deadline) {
                self.expire(calls);
                return;
            }
        }
    }

    /// Marks the deadline as passed, drops the calls that have not started and gives the running
    /// ones [`EXPIRY_GRACE`] to end: the host ends a call that honors its deadline (a `run()`
    /// command is killed) within that time, and the process does not exit before it has.
    fn expire<H: Host>(&self, calls: &RefCell<Calls<H>>) {
        self.expired.store(true, Ordering::Relaxed);
        calls.borrow_mut().drop_queued();
        let until = Instant::now() + EXPIRY_GRACE;
        while !calls.borrow().idle() {
            if !absorb(calls, until) {
                return;
            }
        }
    }

    /// Maps a failure inside the context: the deadline first, then what was thrown.
    pub(crate) fn fail(
        &self,
        ctx: &Ctx<'_>,
        error: Error,
        offset: usize,
        limits: &Limits,
    ) -> ScriptError {
        if self.expired() {
            return ScriptError::Timeout {
                secs: limits.deadline.as_secs(),
                partial: Partial::default(),
            };
        }
        describe(ctx, error, offset, limits)
    }
}

/// Waits for the next call to finish and keeps its value; `false` when `until` passes first.
fn absorb<H: Host>(calls: &RefCell<Calls<H>>, until: Instant) -> bool {
    let Some(done) = calls.borrow_mut().wait(until) else {
        return false;
    };
    calls.borrow_mut().keep(done);
    true
}

/// Attaches what the program printed and what unawaited calls completed to a failed run.
fn with_output<H: Host>(
    sandbox: &Sandbox,
    calls: &RefCell<Calls<H>>,
    error: ScriptError,
) -> ScriptError {
    // The interrupt handler stops every run of JavaScript once the deadline has passed.
    sandbox.runtime.set_interrupt_handler(None);
    let partial = sandbox.context.with(|ctx| Partial {
        prints: printed(&ctx),
        completed: calls.borrow_mut().take_unawaited(),
    });
    error.with_partial(partial)
}

/// Returns the values the program has printed so far, read after the interrupt handler is gone.
fn printed(ctx: &Ctx<'_>) -> Vec<Value> {
    ctx.eval::<String, _>(
        "JSON.stringify(typeof __prints === 'undefined' ? [] : __prints.map(__ser))",
    )
    .ok()
    .and_then(|text| serde_json::from_str(&text).ok())
    .unwrap_or_default()
}

pub(crate) fn throw(ctx: &Ctx<'_>, error: &HostError) -> Error {
    let exception = match Exception::from_message(ctx.clone(), &error.message) {
        Ok(exception) => exception,
        Err(e) => return e,
    };
    if let Some(hint) = &error.hint
        && let Err(e) = exception.as_object().set("hint", hint.as_str())
    {
        return e;
    }
    exception.throw()
}

pub(crate) fn engine(error: &Error) -> ScriptError {
    ScriptError::Engine(error.to_string())
}

fn describe(ctx: &Ctx<'_>, error: Error, offset: usize, limits: &Limits) -> ScriptError {
    let memory = ScriptError::Memory {
        mib: limits.memory >> 20,
        partial: Partial::default(),
    };
    match error {
        Error::Allocation => memory,
        Error::Exception => uncaught(ctx, offset).unwrap_or(memory),
        Error::WouldBlock => {
            ScriptError::Engine("the program waits on a promise that never settles".to_owned())
        }
        other => engine(&other),
    }
}

/// The thrown value as an error, or `None` when it is the engine's out-of-memory error.
fn uncaught(ctx: &Ctx<'_>, offset: usize) -> Option<ScriptError> {
    let thrown = ctx.catch();
    let Some(error) = thrown.as_object() else {
        let shown = ctx
            .json_stringify(thrown.clone())
            .ok()
            .flatten()
            .and_then(|s| s.to_string().ok());
        return Some(ScriptError::Uncaught {
            line: None,
            message: format!("uncaught {}", shown.unwrap_or_default()),
            hint: None,
            partial: Partial::default(),
        });
    };
    let message = property(error, "message").unwrap_or_default();
    if message.contains("out of memory") {
        return None;
    }
    let name = property(error, "name").unwrap_or_else(|| "Error".to_owned());
    Some(ScriptError::Uncaught {
        line: property(error, "stack").and_then(|stack| program_line(&stack, offset)),
        message: if name == "Error" {
            message
        } else {
            format!("{name}: {message}")
        },
        hint: property(error, "hint"),
        partial: Partial::default(),
    })
}

fn property(object: &Object<'_>, key: &str) -> Option<String> {
    object.get::<_, Option<String>>(key).ok().flatten()
}

/// The program line of the innermost stack frame that lies past the engine's own `offset` lines.
fn program_line(stack: &str, offset: usize) -> Option<usize> {
    let frame = Regex::new(r":(\d+):\d+").ok()?;
    frame
        .captures_iter(stack)
        .filter_map(|c| c.get(1)?.as_str().parse::<usize>().ok())
        .find_map(|line| line.checked_sub(offset).filter(|&l| l > 0))
}
