// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn a_matching_permission_request_is_answered_without_asking_the_client() {
    let t0 = Instant::now();
    let mut e = approving(AutoApprove::AllowOnce, "");
    let out = e.on_agent(&permission_request("Read a.rs", "read"), t0);
    assert!(client_lines(&out).is_empty(), "{out:?}");
    let sent = agent_msgs(&out);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["id"], "perm-9");
    assert_eq!(sent[0]["result"]["outcome"]["outcome"], "selected");
    assert_eq!(sent[0]["result"]["outcome"]["optionId"], "once");
    assert!(has_note(&out, "auto-approved permission request"));
    // Nothing is left waiting for the client.
    assert!(e.sessions.get("s").is_none_or(|s| s.asking.is_empty()));

    let mut e = approving(AutoApprove::AllowAlways, "Edit");
    let out = e.on_agent(&permission_request("Edit a.rs", "edit"), t0);
    assert_eq!(
        agent_msgs(&out)[0]["result"]["outcome"]["optionId"],
        "always"
    );
}

#[test]
fn other_permission_requests_are_forwarded_untouched() {
    let t0 = Instant::now();
    let req = permission_request("Bash ls", "execute");
    let mut e = approving(AutoApprove::AllowOnce, "");
    let out = e.on_agent(&req, t0);
    assert!(agent_msgs(&out).is_empty());
    assert!(out.contains(&Action::ToClient {
        line: req.clone(),
        synthesized: false
    }));
    assert!(has_note(&out, "forwarded to the client"));
    assert!(e.sessions["s"].asking.contains("\"perm-9\""));

    // Off by default: no policy, no note.
    let mut e = engine();
    let out = e.on_agent(&permission_request("Read a.rs", "read"), t0);
    assert!(agent_msgs(&out).is_empty());
    assert!(notes(&out).is_empty());
    assert_eq!(client_lines(&out).len(), 1);
}
