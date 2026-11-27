// SPDX-License-Identifier: MPL-2.0
//! The host functions that run the read-only acus commands, and the data functions that a call
//! name routes to. Nothing here changes a file.

use super::super::argv::Call;
use super::super::host;
use super::super::reads;
use super::super::shapes;
use super::Bindings;
use super::args::{internal, one_string, parse};
use super::host_error;
use crate::out::Out;
use crate::{
    Cmd, cmd_blame, cmd_diff, cmd_files, cmd_find, cmd_follow, cmd_log, cmd_outline, cmd_show,
};
use acus_script::HostError;
use capsa_core::format::Format;
use serde_json::{Value, json};
use std::time::Instant;

impl Bindings {
    fn find(&self, mut call: Call) -> Result<Value, HostError> {
        let patterns = std::mem::take(&mut call.positionals);
        call.options.insert("regexp".to_owned(), json!(patterns));
        let files_only = call.flag("filesWithMatches") || call.flag("l");
        let count = call.flag("count") || call.flag("c");
        let block = call.flag("block") || call.flag("b");
        let Cmd::Find(a) = parse("find", &call, &[])? else {
            return Err(internal("find"));
        };
        let report = cmd_find::compute(&a).map_err(|e| host_error(&e))?;
        Ok(if files_only || count {
            shapes::hit_files(&report, count)
        } else if block {
            shapes::blocks(&report)
        } else {
            shapes::hits(&report)
        })
    }

    /// Runs a call that is not a write, MCP call or sleep.
    pub(super) fn query(
        &self,
        name: &str,
        args: Vec<Value>,
        deadline: Instant,
    ) -> Result<Value, HostError> {
        let call = Call::new(name, args)?;
        reads::check(&self.permissions, name, &call)?;
        match name {
            "find" => self.find(call),
            "run" => self.run(call, deadline),
            "read" => self.read(&call),
            "stat" => self.stat(&call),
            "exists" => self.exists(&call),
            "env" => Ok(host::env(one_string(&call, name)?)),
            "fetch" => self.fetch(&call, deadline),
            "readJson" => self.read_json(&call),
            "textDiff" => self.text_diff(&call),
            "parse_toml" => host::parse_toml(one_string(&call, name)?),
            "parse_yaml" => host::parse_yaml(one_string(&call, name)?),
            "parse_csv" => host::parse_csv(
                one_string(&call, name)?,
                call.text("delimiter").unwrap_or(","),
                call.options
                    .get("header")
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
            ),
            "show" => {
                let Cmd::Show(a) = parse(name, &call, &[])? else {
                    return Err(internal(name));
                };
                shapes::views(&cmd_show::compute(&a, Format::Json).map_err(|e| host_error(&e))?)
            }
            "outline" => {
                let Cmd::Outline(a) = parse(name, &call, &[])? else {
                    return Err(internal(name));
                };
                let report = cmd_outline::compute(&a).map_err(|e| host_error(&e))?;
                texted(shapes::outline(&report), || {
                    let _ = cmd_outline::print(&report, &a, &Out::new(Format::Agent));
                })
            }
            "files" => {
                let Cmd::Files(a) = parse(name, &call, &[])? else {
                    return Err(internal(name));
                };
                shapes::files(&cmd_files::compute(&a).map_err(|e| host_error(&e))?)
            }
            "follow" => {
                let Cmd::Follow(a) = parse(name, &call, &[])? else {
                    return Err(internal(name));
                };
                let report = cmd_follow::compute(&a).map_err(|e| host_error(&e))?;
                texted(shapes::follow(&report), || {
                    cmd_follow::print(&report, &Out::new(Format::Agent));
                })
            }
            "diff" => {
                let Cmd::Diff(a) = parse(name, &call, &[])? else {
                    return Err(internal(name));
                };
                let report = cmd_diff::compute(&a, false).map_err(|e| host_error(&e))?;
                texted(shapes::diff(&report), || {
                    cmd_diff::print(&report, &Out::new(Format::Agent), &a);
                })
            }
            "log" => {
                let Cmd::Log(a) = parse(name, &call, &[])? else {
                    return Err(internal(name));
                };
                let report = cmd_log::compute(&a).map_err(|e| host_error(&e))?;
                texted(shapes::log(&report), || {
                    let _ = cmd_log::print(&report, &Out::new(Format::Agent), &a);
                })
            }
            "blame" => {
                let Cmd::Blame(a) = parse(name, &call, &[])? else {
                    return Err(internal(name));
                };
                let report = cmd_blame::compute(&a).map_err(|e| host_error(&e))?;
                texted(shapes::blame(&report), || {
                    cmd_blame::print(&report, &Out::new(Format::Agent));
                })
            }
            _ => Err(internal(name)),
        }
    }
}

/// Adds what `print` writes for the command, as the CLI prints it, to the shaped result as `text`,
/// so a script that prints the result shows the same lines.
fn texted(shape: Result<Value, HostError>, print: impl FnOnce()) -> Result<Value, HostError> {
    let printed = crate::out::capture(print);
    let mut value = shape?;
    let text = printed.trim_end_matches('\n');
    if !text.is_empty() {
        value["text"] = json!(text);
    }
    Ok(value)
}
