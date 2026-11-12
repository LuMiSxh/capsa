// SPDX-License-Identifier: MPL-2.0

use vicarius::bulletin::withdraw;

#[test]
fn withdraw_removes_the_vicarius_bulletin_and_tolerates_a_missing_one() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("vicarius.json");
    std::fs::write(&file, "{}").unwrap();
    withdraw(dir.path()).unwrap();
    assert!(!file.exists());
    withdraw(dir.path()).unwrap();
}
