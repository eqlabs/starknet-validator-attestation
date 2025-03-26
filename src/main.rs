use anyhow::Context;
use jsonrpc::Client;
use starknet::signers::{LocalWallet, SigningKey};
use starknet_crypto::Felt;
use tokio::select;
use tracing_subscriber::prelude::*;
use url::Url;

mod attestation_info;
mod config;
mod events;
mod headers;
mod jsonrpc;
mod metrics_exporter;
mod state;
mod subscription;

const TASK_RESTART_DELAY: std::time::Duration = std::time::Duration::from_secs(5);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    tracing::info!("Starting up");

    let prometheus_handle = metrics_exporter_prometheus::PrometheusBuilder::new()
        .add_global_label("network", "sepolia-testnet")
        .install_recorder()
        .context("Creating Prometheus metrics recorder")?;
    let addr: std::net::SocketAddr = config::METRICS_ADDRESS.parse()?;
    metrics_exporter::spawn(addr, prometheus_handle)
        .await
        .context("Staring metrics exporter")?;

    let node_url = Url::parse(config::NODE_URL_WS)?;

    let signer = LocalWallet::from_signing_key(SigningKey::from_secret_scalar(
        Felt::from_hex(&std::env::var("PRIVATE_KEY").unwrap()).unwrap(),
    ));

    let (new_heads_tx, mut new_heads_rx) = tokio::sync::mpsc::channel(10);
    let mut new_block_fetcher_handle =
        tokio::task::spawn(headers::fetch(node_url.clone(), new_heads_tx.clone()));

    let (events_tx, mut events_rx) = tokio::sync::mpsc::channel(10);
    let mut events_fetcher_handle =
        tokio::task::spawn(events::fetch(node_url.clone(), events_tx.clone()));

    let client = jsonrpc::StarknetRpcClient::new(Url::parse(config::NODE_URL_HTTP)?);
    let attestation_info = client
        .get_attestation_info(config::STAKER_OPERATIONAL_ADDRESS)
        .await
        .context("Getting attestation info")?;
    tracing::debug!(?attestation_info, "Current attestation info");
    let mut state = state::State::from_attestation_info(attestation_info);

    loop {
        select! {
            block_fetcher_result = &mut new_block_fetcher_handle => {
                tracing::error!(error=?block_fetcher_result, "New block fetcher task has exited, restarting");
                let new_block_fetcher_fut = headers::fetch(node_url.clone(), new_heads_tx.clone());
                new_block_fetcher_handle = tokio::task::spawn(async move {
                    tokio::time::sleep(TASK_RESTART_DELAY).await;
                    new_block_fetcher_fut.await
                });
            }
            events_fetcher_result = &mut events_fetcher_handle => {
                tracing::error!(error=?events_fetcher_result, "Events fetcher task has exited, restarting");
                let events_fetcher_fut = events::fetch(node_url.clone(), events_tx.clone());
                events_fetcher_handle = tokio::task::spawn(async move {
                    tokio::time::sleep(TASK_RESTART_DELAY).await;
                    events_fetcher_fut.await
                });
            }
            new_block_header = new_heads_rx.recv() => {
                match new_block_header {
                    Some(header) => {
                        tracing::debug!("Received new block header: {:?}", header);
                        metrics::gauge!("validator_attestation_starknet_latest_block_number").set(header.block_number as f64);

                        // FIXME: handle reorgs
                        let old_state = state.clone();
                        let result = state.handle_new_block_header(&client, &signer, header.block_number, header.block_hash).await;
                        match result {
                            Ok(new_state) => {
                                tracing::debug!(?new_state, "State transition complete");
                                state = new_state;
                            },
                            Err(error) => {
                                tracing::error!(?error, "Failed to handle new block header");
                                state = old_state;
                            }
                        }
                    },
                    None => tracing::warn!("New block header channel closed"),
                }
            }
            event = events_rx.recv() => {
                match event {
                    Some(event) => {
                        tracing::debug!("Received new event: {:?}", event);
                        state = state.handle_new_event(event);
                        tracing::debug!(new_state=?state, "State transition complete");
                    },
                    None => tracing::warn!("New event channel closed"),
                }
            }
        }
    }
}
