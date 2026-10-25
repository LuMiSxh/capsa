// SPDX-License-Identifier: MPL-2.0
//! Owns the life of one request: parse, validate, remember or ask, run, answer, log.

use super::approval::{Approver, Choice, Memory, Prompt};
use super::exec::Executor;
use super::log::Log;
use super::request::{Request, Response};
use super::text::clean;
use super::tools::{Plan, plan};
use std::path::PathBuf;

/// Handles requests for one run, in one working directory.
pub struct Handler {
    pub cwd: PathBuf,
    pub approver: Box<dyn Approver>,
    pub executor: Box<dyn Executor>,
    pub memory: Memory,
    pub log: Log,
}

impl Handler {
    /// Answers one request line. The working directory is the run's, never the request's.
    /// A request is logged before anything else happens; when the log cannot be written,
    /// nothing runs.
    pub fn handle(&self, line: &str) -> Response {
        let request: Request = match serde_json::from_str(line) {
            Ok(request) => request,
            Err(err) => {
                let message = format!("bad request: {err}");
                let _ = self.log.write(&clean(&message));
                return Response::error(message);
            }
        };
        let plan = match plan(&*self.executor, &self.cwd, &request) {
            Ok(plan) => plan,
            Err(refusal) => {
                let _ = self.log.write(&clean(&format!("refused: {refusal}")));
                return Response::error(refusal.to_string());
            }
        };
        if let Err(err) = self.log.write(&format!(
            "request {} repo={} {}",
            plan.tool,
            clean(&plan.repo.to_string_lossy()),
            summary(&plan)
        )) {
            return Response::error(format!("the broker log cannot be written: {err}"));
        }
        let Some(how) = self.decide(&plan) else {
            let _ = self.log.write("decision deny");
            return Response::denied();
        };
        let _ = self.log.write(&format!("decision allow ({how})"));
        self.execute(&plan)
    }

    /// Returns how the request was allowed, or `None` for a denial.
    fn decide(&self, plan: &Plan) -> Option<&'static str> {
        if self.memory.covers(plan.gate, &plan.repo) {
            return Some("remembered");
        }
        let prompt = Prompt {
            tool: plan.tool,
            repo: plan.repo.clone(),
            details: plan.details.clone(),
            remember: plan.gate.remember,
        };
        let choice = self.approver.ask(&prompt);
        if !choice.offered(plan.gate.remember) {
            return None;
        }
        if let Err(err) = self.memory.remember(plan.gate, &plan.repo, choice) {
            let _ = self
                .log
                .write(&clean(&format!("could not store the approval: {err}")));
        }
        match choice {
            Choice::Deny => None,
            Choice::Once => Some("once"),
            Choice::Session => Some("session"),
            Choice::Repo => Some("repo"),
        }
    }

    fn execute(&self, plan: &Plan) -> Response {
        match self.executor.run(&self.cwd, &plan.argv) {
            Ok(ran) => {
                let _ = self
                    .log
                    .write(&format!("result {} exit={}", plan.tool, ran.code));
                Response::done(ran.code, ran.output)
            }
            Err(err) => {
                let _ = self.log.write(&format!("result {} not started", plan.tool));
                Response::error(format!("could not start {}: {err}", plan.argv[0]))
            }
        }
    }
}

/// Returns the plan's details as one clean log line.
fn summary(plan: &Plan) -> String {
    plan.details
        .iter()
        .map(|(label, value)| format!("{label}={}", clean(value)))
        .collect::<Vec<_>>()
        .join(" ")
}
