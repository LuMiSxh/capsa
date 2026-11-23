// SPDX-License-Identifier: MPL-2.0
//! Applies the `Read(...)` rules to the paths a script names in `show`, `outline`, `blame`, `follow`
//! and `find`. Directories are not checked: a rule names a file, and a walk is not an explicit
//! request for one.

use super::argv::Call;
use crate::source;
use acus_script::HostError;
use capsa_core::claude_settings::permissions::Permissions;
use serde_json::Value;
use std::path::Path;

/// Refuses a call that names a file a `Read(...)` rule denies or asks about.
pub(super) fn check(perms: &Permissions, name: &str, call: &Call) -> Result<(), HostError> {
    let mut named: Vec<String> = Vec::new();
    if matches!(name, "show" | "outline" | "blame" | "follow") {
        named.extend(call.positionals.iter().cloned());
    }
    if matches!(name, "show" | "outline" | "blame" | "follow" | "find") {
        match call.options.get("paths") {
            Some(Value::String(path)) => named.push(path.clone()),
            Some(Value::Array(items)) => {
                named.extend(items.iter().filter_map(Value::as_str).map(str::to_owned));
            }
            _ => {}
        }
    }
    for address in &named {
        let path = file_of(address);
        if !Path::new(&path).is_dir() {
            perms.check_read(Path::new(&path)).map_err(super::refused)?;
        }
    }
    Ok(())
}

/// The file an address names: without its `#Symbol` or line range, and for `REV:path` the path.
fn file_of(address: &str) -> String {
    let plain = match source::revision(address) {
        Some((_, file)) => file,
        None => address,
    };
    source::address(plain).path
}
