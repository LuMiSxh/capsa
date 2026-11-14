// SPDX-License-Identifier: MPL-2.0
//! Runs the balance refresh and the agent sync against a mock balance endpoint.

#![expect(
    clippy::unwrap_used,
    reason = "setup helpers outside a test fn must fail the test"
)]

#[expect(dead_code, reason = "the proxy tests use more of the shared mock")]
#[path = "../proxy/mock.rs"]
mod mock;

use mock::{Mock, Reply, Step};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::Notify;
use vicarius::config::{Agent, Config, Effort};
use vicarius::gate::{Closed, Gate};
use vicarius::proxy::{DeepseekKey, Proxy, ProxyConfig};
use vicarius::refresh::{RefreshConfig, Refresher};

const KEY: &str = "sk-REFRESHSECRET777";
const SOURCE: &str =
    "---\nname: senior-worker\ndescription: Does the work\nmodel: sonnet\n---\nBody\n";

struct Rig {
    refresher: Arc<Refresher>,
    gate: Arc<Gate>,
    tmp: tempfile::TempDir,
    agents: PathBuf,
    config_file: PathBuf,
    log: PathBuf,
    bulletin: PathBuf,
}

fn rig(base: &str, key: Option<&str>) -> Rig {
    let tmp = tempfile::tempdir().unwrap();
    let agents = tmp.path().join("agents");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(agents.join("senior-worker.md"), SOURCE).unwrap();
    let key = key.and_then(DeepseekKey::new);
    let gate = Arc::new(Gate::new(key.is_some()));
    let mut config = RefreshConfig::new(key);
    config.balance_base = base.to_owned();
    config.config_file = tmp.path().join("config.toml");
    config.agents_dir = agents.clone();
    config.log_file = tmp.path().join("proxy.jsonl");
    config.sync_every = Duration::from_millis(100);
    config.bulletin_dir = tmp.path().join("bulletin");
    let refresher = Arc::new(Refresher::new(config.clone(), gate.clone()).unwrap());
    Rig {
        refresher,
        gate,
        agents,
        config_file: config.config_file,
        log: config.log_file,
        bulletin: config.bulletin_dir,
        tmp,
    }
}

fn balance_body(available: bool, usd: &str) -> String {
    format!(
        r#"{{"is_available":{available},"balance_infos":[{{"currency":"USD","total_balance":"{usd}","granted_balance":"0.00","topped_up_balance":"{usd}"}}]}}"#
    )
}

async fn balance_mock(status: u16, body: String) -> Mock {
    Mock::start(move |_| {
        Reply::full(
            status,
            &[("content-type", b"application/json")],
            body.as_bytes(),
        )
    })
    .await
}

async fn wait_for(path: PathBuf, present: bool) {
    for _ in 0..200 {
        if path.exists() == present {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("{} never became present={present}", path.display());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_available_balance_opens_the_gate() {
    let mock = balance_mock(200, balance_body(true, "5.00")).await;
    let rig = rig(&mock.url(), Some(KEY));
    rig.refresher.refresh().await;
    let snap = rig.gate.snapshot();
    assert!(snap.is_open());
    assert_eq!(
        (snap.balance, snap.currency.as_deref()),
        (Some(5.0), Some("USD"))
    );
    let seen = mock.seen();
    assert_eq!(seen[0].target, "/user/balance");
    assert_eq!(seen[0].headers["authorization"], format!("Bearer {KEY}"));
    let (_, view) = rig.refresher.view();
    assert!(view.last_ok_at.is_some() && view.next_at.is_some() && view.last_error.is_none());
    let log = std::fs::read_to_string(&rig.log).unwrap();
    assert!(
        log.contains(r#""event":"balance.refresh""#) && log.contains(r#""outcome":"available""#)
    );
    assert!(!log.contains(KEY));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_low_balance_closes_the_gate_and_keeps_the_amount() {
    let mock = balance_mock(200, balance_body(true, "0.40")).await;
    let rig = rig(&mock.url(), Some(KEY));
    rig.gate.open(9.0, "USD", 1);
    rig.refresher.refresh().await;
    let snap = rig.gate.snapshot();
    assert_eq!(snap.closed, Some(Closed::LowBalance { minimum: 1.0 }));
    assert_eq!(snap.balance, Some(0.4));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unavailable_account_closes_the_gate() {
    let mock = balance_mock(200, balance_body(false, "5.00")).await;
    let rig = rig(&mock.url(), Some(KEY));
    rig.refresher.refresh().await;
    assert_eq!(rig.gate.closed(), Some(Closed::Unavailable));
}

#[tokio::test(flavor = "multi_thread")]
async fn http_401_and_402_map_to_their_reasons() {
    for (status, reason) in [(401, Closed::KeyRejected), (402, Closed::PaymentRequired)] {
        let mock = balance_mock(status, "{}".to_owned()).await;
        let rig = rig(&mock.url(), Some(KEY));
        rig.gate.open(9.0, "USD", 1);
        rig.refresher.refresh().await;
        assert_eq!(rig.gate.closed(), Some(reason));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_network_error_keeps_the_gate_and_records_the_error() {
    let rig = rig("http://127.0.0.1:1", Some(KEY));
    rig.refresher.refresh().await;
    assert_eq!(rig.gate.closed(), Some(Closed::NotChecked));
    rig.gate.open(2.0, "USD", 1);
    rig.refresher.refresh().await;
    assert!(rig.gate.snapshot().is_open());
    let (_, view) = rig.refresher.view();
    assert!(view.last_error.is_some() && view.last_ok_at.is_none());
    let log = std::fs::read_to_string(&rig.log).unwrap();
    assert!(log.contains(r#""outcome":"error""#) && !log.contains(KEY));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refresh_reopens_a_gate_the_proxy_closed_on_402() {
    let mock = balance_mock(200, balance_body(true, "7.00")).await;
    let rig = rig(&mock.url(), Some(KEY));
    rig.gate.close(Closed::PaymentRequired, 1);
    rig.refresher.refresh().await;
    assert!(rig.gate.snapshot().is_open());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_read_that_started_before_a_402_cannot_reopen_the_gate() {
    let release = Arc::new(Notify::new());
    let calls = Arc::new(AtomicUsize::new(0));
    let (held, count) = (release.clone(), calls.clone());
    let mock = Mock::start(move |_| {
        let steps = if count.fetch_add(1, Ordering::SeqCst) == 0 {
            vec![
                Step::Wait(held.clone()),
                Step::Data(balance_body(true, "7.00").into_bytes()),
            ]
        } else {
            vec![Step::Data(balance_body(true, "7.00").into_bytes())]
        };
        Reply::steps(200, &[("content-type", b"application/json")], steps)
    })
    .await;
    let rig = rig(&mock.url(), Some(KEY));
    let stale = tokio::spawn({
        let refresher = rig.refresher.clone();
        async move { refresher.refresh().await }
    });
    while mock.seen().is_empty() {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    rig.gate.close(Closed::PaymentRequired, 5);
    release.notify_one();
    stale.await.unwrap();
    let snap = rig.gate.snapshot();
    assert_eq!(snap.closed, Some(Closed::PaymentRequired));
    assert_eq!(snap.balance, Some(7.0));

    rig.refresher.refresh().await;
    assert!(rig.gate.snapshot().is_open());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refresh_that_overlaps_another_runs_once() {
    let release = Arc::new(Notify::new());
    let held = release.clone();
    let mock = Mock::start(move |_| {
        Reply::steps(
            200,
            &[("content-type", b"application/json")],
            vec![
                Step::Wait(held.clone()),
                Step::Data(balance_body(true, "5.00").into_bytes()),
            ],
        )
    })
    .await;
    let rig = rig(&mock.url(), Some(KEY));
    let first = tokio::spawn({
        let refresher = rig.refresher.clone();
        async move { refresher.refresh().await }
    });
    while mock.seen().is_empty() {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let second = tokio::spawn({
        let refresher = rig.refresher.clone();
        async move { refresher.refresh().await }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    release.notify_one();
    first.await.unwrap();
    second.await.unwrap();

    assert_eq!(mock.seen().len(), 1);
    let log = std::fs::read_to_string(&rig.log).unwrap();
    assert_eq!(
        log.matches(r#""event":"balance.refresh""#).count(),
        1,
        "{log}"
    );
    assert!(rig.gate.snapshot().is_open());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_repeat_refresh_reuses_a_recent_success() {
    let mock = balance_mock(200, balance_body(true, "5.00")).await;
    let rig = rig(&mock.url(), Some(KEY));
    rig.refresher.refresh().await;
    rig.refresher.refresh().await;

    assert_eq!(mock.seen().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn without_a_key_nothing_is_requested() {
    let mock = balance_mock(200, balance_body(true, "5.00")).await;
    let rig = rig(&mock.url(), None);
    rig.refresher.refresh_and_sync().await;
    assert!(mock.seen().is_empty());
    assert_eq!(rig.gate.closed(), Some(Closed::NoKey));
    assert!(!rig.agents.join("senior-worker-ds.md").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn sync_writes_the_agent_when_open_and_removes_it_when_closed() {
    let rig = rig("http://127.0.0.1:1", Some(KEY));
    let derived = rig.agents.join("senior-worker-ds.md");
    rig.gate.open(5.0, "USD", 1);
    rig.refresher.sync_agents();
    assert!(derived.exists());
    rig.gate.close(Closed::PaymentRequired, 2);
    rig.refresher.sync_agents();
    assert!(!derived.exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn sync_reads_the_config_again_for_added_agents() {
    let rig = rig("http://127.0.0.1:1", Some(KEY));
    rig.gate.open(5.0, "USD", 1);
    rig.refresher.sync_agents();
    std::fs::write(
        rig.agents.join("worker.md"),
        SOURCE.replace("senior-worker", "worker"),
    )
    .unwrap();
    let mut config = Config::default();
    config.agents.push(Agent {
        from: "worker".to_owned(),
        model: "deepseek-flash".to_owned(),
        effort: Effort::Max,
        suspended: false,
    });
    config.save_to(&rig.config_file).unwrap();
    rig.refresher.sync_agents();
    let derived = std::fs::read_to_string(rig.agents.join("worker-ds.md")).unwrap();
    assert!(
        derived.lines().any(|line| line == "effort: max"),
        "{derived}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_derived_file_is_removed_when_its_source_disappears() {
    let rig = rig("http://127.0.0.1:1", Some(KEY));
    rig.gate.open(5.0, "USD", 1);
    rig.refresher.sync_agents();
    std::fs::remove_file(rig.agents.join("senior-worker.md")).unwrap();
    rig.refresher.sync_agents();
    assert!(!rig.agents.join("senior-worker-ds.md").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn run_syncs_at_start_and_withdraws_the_agents_on_shutdown() {
    let mock = balance_mock(200, balance_body(true, "5.00")).await;
    let rig = rig(&mock.url(), Some(KEY));
    let derived = rig.agents.join("senior-worker-ds.md");
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(rig.refresher.clone().run(stopped));
    wait_for(derived.clone(), true).await;
    stop.send(true).unwrap();
    task.await.unwrap();
    assert!(!derived.exists());
    assert!(rig.tmp.path().exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn post_refresh_returns_the_updated_status() {
    let mock = balance_mock(200, balance_body(true, "5.00")).await;
    let rig = rig(&mock.url(), Some(KEY));
    let mut config = ProxyConfig::new(0, DeepseekKey::new(KEY));
    config.log_file = rig.tmp.path().join("proxy.jsonl");
    let proxy = Proxy::bind(config, rig.gate.clone())
        .await
        .unwrap()
        .with_refresher(rig.refresher.clone());
    let addr = proxy.local_addr().unwrap();
    tokio::spawn(proxy.run(std::future::pending()));
    let client = reqwest::Client::new();
    let url = format!("http://{addr}/vicarius");

    let before: serde_json::Value = serde_json::from_slice(
        &client
            .get(format!("{url}/status"))
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(before["gate"]["open"], false);
    assert!(before["agents"].is_null());
    assert!(before["refresh"]["last_ok_at"].is_null());

    let reply = client.post(format!("{url}/refresh")).send().await.unwrap();
    assert_eq!(reply.status(), 200);
    let after: serde_json::Value = serde_json::from_slice(&reply.bytes().await.unwrap()).unwrap();
    assert_eq!(after["gate"]["open"], true);
    assert_eq!(after["gate"]["balance"], 5.0);
    assert_eq!(after["agents"]["entries"][0]["file"], "senior-worker-ds.md");
    assert_eq!(after["agents"]["entries"][0]["outcome"], "written");
    assert!(after["refresh"]["last_ok_at"].is_u64());
    assert!(after["refresh"]["next_at"].is_u64());
    assert!(after["refresh"]["last_error"].is_null());
    assert!(rig.agents.join("senior-worker-ds.md").exists());
}

fn published(rig: &Rig) -> serde_json::Value {
    let text = std::fs::read_to_string(rig.bulletin.join("vicarius.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

async fn wait_for_open(rig: &Rig, open: bool) -> serde_json::Value {
    for _ in 0..200 {
        let written = std::fs::read_to_string(rig.bulletin.join("vicarius.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
        if let Some(value) = written.filter(|v| v["deepseek"]["value"]["open"] == open) {
            return value;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the bulletin never showed open={open}");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_available_balance_publishes_the_open_route_with_its_balance() {
    let mock = balance_mock(200, balance_body(true, "5.00")).await;
    let rig = rig(&mock.url(), Some(KEY));
    rig.refresher.refresh().await;
    let entry = &published(&rig)["deepseek"];
    assert_eq!(
        entry["value"],
        serde_json::json!({"open": true, "balance_usd": 5.0})
    );
    assert_eq!(entry["ttl"], 1200);
    assert!(entry["at"].is_u64());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_402_on_refresh_publishes_the_closed_route() {
    let mock = balance_mock(402, "{}".to_owned()).await;
    let rig = rig(&mock.url(), Some(KEY));
    rig.gate.open(9.0, "USD", 1);
    rig.refresher.refresh().await;
    assert_eq!(
        published(&rig)["deepseek"]["value"],
        serde_json::json!({"open": false, "balance_usd": 9.0})
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_402_from_a_proxied_request_publishes_the_closed_route() {
    let deepseek = Mock::start(|_| Reply::full(402, &[], b"insufficient balance")).await;
    let rig = rig("http://127.0.0.1:1", Some(KEY));
    rig.gate.open(9.0, "USD", 1);
    let mut config = ProxyConfig::new(0, DeepseekKey::new(KEY));
    config.deepseek_base = format!("{}/anthropic", deepseek.url());
    config.log_file = rig.tmp.path().join("proxy.jsonl");
    let proxy = Proxy::bind(config, rig.gate.clone())
        .await
        .unwrap()
        .with_refresher(rig.refresher.clone());
    let addr = proxy.local_addr().unwrap();
    tokio::spawn(proxy.run(std::future::pending()));
    let reply = reqwest::Client::new()
        .post(format!("http://{addr}/v1/messages"))
        .body(r#"{"model":"deepseek-flash","messages":[]}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(reply.status(), 400);
    let value = wait_for_open(&rig, false).await;
    assert_eq!(value["deepseek"]["value"]["balance_usd"], 9.0);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_published_ttl_is_twice_the_balance_refresh_period() {
    let mock = balance_mock(200, balance_body(true, "5.00")).await;
    let rig = rig(&mock.url(), Some(KEY));
    std::fs::write(&rig.config_file, "balance_refresh_minutes = 3\n").unwrap();
    rig.refresher.refresh().await;
    assert_eq!(published(&rig)["deepseek"]["ttl"], 360);
}
