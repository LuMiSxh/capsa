// SPDX-License-Identifier: MPL-2.0

use serde_json::json;

use super::harness::Harness;

#[tokio::test]
async fn a_permission_request_for_a_covered_tool_is_answered_by_pons() {
    let mut h = Harness::start(&["--auto-approve", "allow-once"]);
    h.prompt(1, "permission-read").await;
    let seen = h
        .recv_until(|m| m["method"] == "mock/permission_seen")
        .await;
    assert!(
        seen.iter()
            .all(|m| m["method"] != "session/request_permission"),
        "the client was not asked: {seen:?}"
    );
    let result = &seen.last().unwrap()["params"]["result"];
    assert_eq!(result["outcome"]["outcome"], "selected");
    assert_eq!(result["outcome"]["optionId"], "allow-once");
}

#[tokio::test]
async fn a_permission_request_for_another_tool_still_reaches_the_client() {
    let mut h = Harness::start(&["--auto-approve", "allow-always"]);
    h.prompt(1, "permission-bash").await;
    let seen = h
        .recv_until(|m| m["method"] == "session/request_permission")
        .await;
    assert_eq!(seen.last().unwrap()["id"], "perm-2");
    h.send(json!({"jsonrpc": "2.0", "id": "perm-2",
        "result": {"outcome": {"outcome": "selected", "optionId": "reject"}}}))
        .await;
    let seen = h
        .recv_until(|m| m["method"] == "mock/permission_seen")
        .await;
    assert_eq!(
        seen.last().unwrap()["params"]["result"]["outcome"]["optionId"],
        "reject"
    );
}

#[tokio::test]
async fn a_listed_tool_is_approved_with_the_policys_option() {
    let mut h = Harness::start(&[
        "--auto-approve",
        "allow-always",
        "--auto-approve-tools",
        "Bash",
    ]);
    h.prompt(1, "permission-bash").await;
    let seen = h
        .recv_until(|m| m["method"] == "mock/permission_seen")
        .await;
    assert_eq!(
        seen.last().unwrap()["params"]["result"]["outcome"]["optionId"],
        "allow-always"
    );
}

#[tokio::test]
async fn permission_requests_are_forwarded_by_default() {
    let mut h = Harness::start(&[]);
    h.prompt(1, "permission-read").await;
    let seen = h
        .recv_until(|m| m["method"] == "session/request_permission")
        .await;
    assert_eq!(seen.last().unwrap()["id"], "perm-2");
}
