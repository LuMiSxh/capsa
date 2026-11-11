// SPDX-License-Identifier: MPL-2.0
//! `vicarius serve`: runs the proxy in the foreground until SIGTERM or SIGINT. The refresher task
//! owns the balance refresh, the gate and the derived agent files; shutdown withdraws the agents.

use anyhow::{Context, Result};
use capsa_core::exit::Outcome;
use capsa_core::format::Format;
use capsa_core::hint::ResultExt;
use std::io::Write;
use std::sync::Arc;
use tokio::sync::watch;
use vicarius::config::Config;
use vicarius::gate::Gate;
use vicarius::proxy::{DeepseekKey, Proxy, ProxyConfig};
use vicarius::refresh::{RefreshConfig, Refresher};

/// The environment variable that holds the DeepSeek API key.
const KEY_VARIABLE: &str = "DEEPSEEK_API_KEY";

#[derive(clap::Args)]
pub struct Args {}

pub fn run(_: &Args, _: Format) -> Result<Outcome> {
    let config = Config::load_from(&Config::path())
        .map_err(anyhow::Error::from)
        .context("cannot load the vicarius config")?;
    let key = std::env::var(KEY_VARIABLE)
        .ok()
        .and_then(|value| DeepseekKey::new(&value));
    let gate = Arc::new(Gate::new(key.is_some()));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("cannot start the async runtime")?;
    runtime.block_on(async {
        let refresher = Refresher::new(RefreshConfig::new(key.clone()), gate.clone())
            .map(Arc::new)
            .map_err(anyhow::Error::from)
            .context("cannot set up the balance refresh")?;
        let proxy = Proxy::bind(ProxyConfig::new(config.port, key), gate)
            .await
            .hinted()?
            .with_refresher(refresher.clone());
        let addr = proxy
            .local_addr()
            .context("cannot read the listening address")?;
        let _ = writeln!(std::io::stderr(), "vicarius: listening on http://{addr}");
        let (stop_tx, stop_rx) = watch::channel(false);
        tokio::spawn(async move {
            capsa_core::process::shutdown_signal().await;
            let _ = stop_tx.send(true);
        });
        let refreshing = tokio::spawn(refresher.run(stop_rx.clone()));
        proxy.run(stopped(stop_rx)).await;
        let _ = refreshing.await;
        Ok(Outcome::Found)
    })
}

/// Completes once the stop flag turns true.
async fn stopped(mut stop: watch::Receiver<bool>) {
    let _ = stop.wait_for(|stopped| *stopped).await;
}
