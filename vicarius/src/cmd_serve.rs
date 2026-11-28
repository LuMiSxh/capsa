// SPDX-License-Identifier: MPL-2.0
//! `vicarius serve`: runs the proxy in the foreground until SIGTERM or SIGINT. One refresher task
//! per gated provider owns the balance refresh, the gate and, for DeepSeek, the derived agents;
//! shutdown withdraws them.

use anyhow::{Context, Result};
use capsa_core::exit::Outcome;
use capsa_core::format::Format;
use capsa_core::hint::ResultExt;
use std::io::Write;
use std::sync::Arc;
use tokio::sync::watch;
use vicarius::config::Config;
use vicarius::gate::Gate;
use vicarius::proxy::{ProviderId, ProviderKey, ProviderSpec, Proxy, ProxyConfig};
use vicarius::refresh::{RefreshConfig, Refresher};

/// The providers the proxy needs and the refreshers it runs, by provider.
type Runtime = (Vec<ProviderSpec>, Vec<(ProviderId, Arc<Refresher>)>);

#[derive(clap::Args)]
pub struct Args {}

pub fn run(_: &Args, _: Format) -> Result<Outcome> {
    let config = Config::load_from(&Config::path())
        .map_err(anyhow::Error::from)
        .context("cannot load the vicarius config")?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("cannot start the async runtime")?;
    runtime.block_on(async {
        let (specs, refreshers) = runtime_providers(&config)?;
        let mut proxy = Proxy::bind(ProxyConfig::new(config.port, specs))
            .await
            .hinted()?;
        for (id, refresher) in &refreshers {
            proxy = proxy.with_refresher(id, refresher.clone());
        }
        let addr = proxy
            .local_addr()
            .context("cannot read the listening address")?;
        let _ = writeln!(std::io::stderr(), "vicarius: listening on http://{addr}");
        let (stop_tx, stop_rx) = watch::channel(false);
        tokio::spawn(async move {
            capsa_core::process::shutdown_signal().await;
            let _ = stop_tx.send(true);
        });
        let refreshing: Vec<_> = refreshers
            .iter()
            .map(|(_, refresher)| tokio::spawn(refresher.clone().run(stop_rx.clone())))
            .collect();
        proxy.run(stopped(stop_rx)).await;
        for task in refreshing {
            let _ = task.await;
        }
        Ok(Outcome::Found)
    })
}

/// Builds one proxy spec and, for each provider with a balance block, its refresher.
fn runtime_providers(config: &Config) -> Result<Runtime> {
    let mut specs = Vec::new();
    let mut refreshers = Vec::new();
    for (name, provider) in &config.providers {
        let id = ProviderId::new(name.clone());
        let key = std::env::var(&provider.key_env)
            .ok()
            .and_then(|value| ProviderKey::new(&value));
        let gate = provider
            .balance
            .as_ref()
            .map(|_| Arc::new(Gate::new(key.is_some())));
        if let Some(gate) = &gate {
            // The `-ds` agent derivation stays DeepSeek-only until providers have their own
            // suffixes.
            let derives_agents = name == "deepseek";
            let refresher = Refresher::new(
                RefreshConfig::new(id.clone(), key.clone(), derives_agents),
                gate.clone(),
            )
            .map(Arc::new)
            .map_err(anyhow::Error::from)
            .context("cannot set up the balance refresh")?;
            refreshers.push((id.clone(), refresher));
        }
        specs.push(ProviderSpec {
            id,
            prefixes: provider.prefixes.clone(),
            base_url: provider.base_url.clone(),
            key_env: provider.key_env.clone(),
            auth: provider.auth,
            models: provider.models.clone(),
            gate,
            compat: provider.compat,
            key,
        });
    }
    Ok((specs, refreshers))
}

/// Completes once the stop flag turns true.
async fn stopped(mut stop: watch::Receiver<bool>) {
    let _ = stop.wait_for(|stopped| *stopped).await;
}
