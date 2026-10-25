// SPDX-License-Identifier: MPL-2.0
//! Owns the life of one request: parse, validate, remember or ask, run, answer, log.

use super::approval::{Approver, Choice, Memory, Prompt};
use super::exec::Executor;
use super::log::Log;
use super::repo::Recorded;
use super::request::{Request, Response};
use super::text::clean;
use super::tools::{Plan, plan};
use std::path::PathBuf;
use std::sync::{Mutex, TryLockError};

/// Handles requests for one run, in one working directory. One request at a time may ask or
/// run; another that gets that far is told to try again.
pub struct Handler {
    pub cwd: PathBuf,
    /// The repository recorded at the start of the run, or why there is none.
    pub repo: Result<Recorded, String>,
    pub approver: Box<dyn Approver>,
    pub executor: Box<dyn Executor>,
    pub memory: Memory,
    pub log: Log,
    pub busy: Mutex<()>,
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
        let plan = match plan(&*self.executor, &self.cwd, &self.repo, &request) {
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
        let _slot = match self.busy.try_lock() {
            Ok(slot) => slot,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => {
                let _ = self.log.write("decision busy");
                return Response::error(
                    "busy: another request is waiting for approval or running; try again later",
                );
            }
        };
        let Some(how) = self.decide(&plan) else {
            let _ = self.log.write("decision deny");
            return Response::denied();
        };
        let _ = self.log.write(&format!("decision allow ({how})"));
        self.execute(&plan)
    }

    /// Returns how the request was allowed, or `None` for a denial.
    fn decide(&self, plan: &Plan) -> Option<&'static str> {
        if self.memory.covers(plan.gate, &plan.repo, &plan.subject) {
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
        if let Err(err) = self
            .memory
            .remember(plan.gate, &plan.repo, &plan.subject, choice)
        {
            let _ = self
                .log
                .write(&clean(&format!("could not store the approval: {err}")));
        }
        match choice {
            Choice::Deny => None,
            Choice::Once => Some("once"),
            Choice::Repo => Some("repo"),
        }
    }

    fn execute(&self, plan: &Plan) -> Response {
        if let Ok(repo) = &self.repo
            && let Err(refusal) = repo.verify()
        {
            let _ = self.log.write(&clean(&format!("refused: {refusal}")));
            return Response::error(refusal.to_string());
        }
        let repo = self.repo.as_ref().ok().filter(|_| plan.bound);
        let cwd = repo.map_or(self.cwd.as_path(), |repo| repo.root.as_path());
        match self.executor.run(cwd, &plan.argv, repo) {
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
