// SPDX-License-Identifier: MPL-2.0
//! The host functions that read files, fetch pages and diff texts. None of them changes a file.

use super::super::argv::Call;
use super::super::host::{self, ReadOptions};
use super::super::net::{self, FetchConfig, FetchOpts};
use super::Bindings;
use super::args::{count_option, one_string};
use acus_script::HostError;
use serde_json::{Value, json};
use std::time::Instant;

impl Bindings {
    /// Reads a file under the repository or an `additionalDirectories` entry, as
    /// `read(path, {offset, limit, numbered})`.
    pub(super) fn read(&self, call: &Call) -> Result<Value, HostError> {
        let options = ReadOptions {
            offset: count_option(call, "offset")?,
            limit: count_option(call, "limit")?,
            numbered: call.flag("numbered"),
        };
        host::read(
            &self.permissions,
            &self.cwd,
            one_string(call, "read")?,
            &options,
        )
    }

    pub(super) fn stat(&self, call: &Call) -> Result<Value, HostError> {
        host::stat(&self.permissions, &self.cwd, one_string(call, "stat")?)
    }

    pub(super) fn exists(&self, call: &Call) -> Result<Value, HostError> {
        host::exists(&self.permissions, &self.cwd, one_string(call, "exists")?).map(Value::Bool)
    }

    /// Fetches an http or https page, as `fetch(url, {grep, head})`. Redirects count against the
    /// time left before the script's deadline.
    pub(super) fn fetch(&self, call: &Call, deadline: Instant) -> Result<Value, HostError> {
        let [url] = call.positionals.as_slice() else {
            return Err(HostError::new("fetch() takes one URL")
                .with_hint("fetch('https://example.com/page', {grep: 'TODO', head: 20})"));
        };
        let opts = FetchOpts {
            grep: call.text("grep").map(str::to_owned),
            head: count_option(call, "head")?,
        };
        let cfg = FetchConfig {
            deny: self.cfg.script.fetch_deny.clone(),
        };
        let left = deadline.saturating_duration_since(Instant::now());
        let page = net::fetch(&self.permissions, &cfg, url, opts, left)?;
        Ok(json!({"status": page.status, "url": page.url, "text": page.text}))
    }

    pub(super) fn text_diff(&self, call: &Call) -> Result<Value, HostError> {
        let [old, new] = call.positionals.as_slice() else {
            return Err(HostError::new("textDiff() takes two texts")
                .with_hint("textDiff(old, new, {context: 3})"));
        };
        let context = count_option(call, "context")?;
        Ok(json!(host::text_diff(old, new, context)))
    }
}
