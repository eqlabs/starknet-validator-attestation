use std::cmp::Ordering;

use anyhow::Context;
use starknet::{
    core::types::{BlockId, MaybePendingBlockWithTxHashes},
    providers::{
        JsonRpcClient, Provider,
        jsonrpc::{HttpTransport, JsonRpcTransport},
    },
    signers::{LocalWallet, SigningKey},
};
use starknet_crypto::Felt;
use tokio::select;
use tracing_subscriber::prelude::*;
use url::Url;

mod attest;
mod attestation_info;
mod config;
mod events;
mod headers;
mod metrics_exporter;
mod subscription;

const TASK_RESTART_DELAY: std::time::Duration = std::time::Duration::from_secs(5);

#[derive(Debug)]
struct AttestationParams {
    block_hash: Felt,
    start_of_attestation_window: u64,
    end_of_attestation_window: u64,
}

impl AttestationParams {
    pub fn in_window(&self, block_number: u64) -> bool {
        block_number >= self.start_of_attestation_window
            && block_number < self.end_of_attestation_window
    }
}

#[derive(Debug)]
enum State {
    WaitingForBlockToAttest {
        attestation_info: attestation_info::AttestationInfo,
        block_to_attest: u64,
    },
    WaitingForAttestationWindow {
        attestation_info: attestation_info::AttestationInfo,
        attestation_params: AttestationParams,
    },
    WaitingForNextEpoch {
        attestation_info: attestation_info::AttestationInfo,
    },
}

impl State {
    pub fn from_attestation_info(
        attestation_info: attestation_info::AttestationInfo,
    ) -> anyhow::Result<Self> {
        let block_to_attest = attestation_info
            .calculate_expected_attestation_block()
            .context("Calculating expected attestation block")?;
        Ok(State::WaitingForBlockToAttest {
            attestation_info,
            block_to_attest,
        })
    }

    fn attestation_info(&self) -> &attestation_info::AttestationInfo {
        match self {
            State::WaitingForBlockToAttest {
                attestation_info, ..
            } => attestation_info,
            State::WaitingForAttestationWindow {
                attestation_info, ..
            } => attestation_info,
            State::WaitingForNextEpoch { attestation_info } => attestation_info,
        }
    }

    fn block_in_current_epoch(&self, block_number: u64) -> bool {
        let attestation_info = self.attestation_info();
        block_number >= attestation_info.current_epoch_starting_block
            && block_number
                < attestation_info.current_epoch_starting_block + attestation_info.epoch_len
    }

    pub async fn handle_new_block_header<T: JsonRpcTransport + Send + Sync + 'static>(
        self,
        provider: &JsonRpcClient<T>,
        signer: &LocalWallet,
        block_number: u64,
        block_hash: Felt,
    ) -> anyhow::Result<Self> {
        // Check if a new epoch has started and re-initialize
        let state = if self.block_in_current_epoch(block_number) {
            self
        } else {
            let attestation_info = attestation_info::get_attestation_info(
                provider,
                config::STAKER_OPERATIONAL_ADDRESS,
            )
            .await
            .context("Getting attestation info")?;
            tracing::debug!(?attestation_info, "New epoch started");
            State::from_attestation_info(attestation_info)?
        };

        Ok(match state {
            State::WaitingForBlockToAttest {
                attestation_info,
                block_to_attest,
            } => match block_number.cmp(&block_to_attest) {
                // Not there yet.
                Ordering::Less => State::WaitingForBlockToAttest {
                    attestation_info,
                    block_to_attest,
                },
                // We have received the block hash for the block to attest.
                Ordering::Equal => {
                    let attestation_window = attestation_info.attestation_window;
                    State::WaitingForAttestationWindow {
                        attestation_info,
                        attestation_params: AttestationParams {
                            block_hash,
                            start_of_attestation_window: block_number
                                + config::MIN_ATTESTATION_WINDOW,
                            end_of_attestation_window: block_number + attestation_window as u64,
                        },
                    }
                }
                // We're past the block on the block header subscription.
                Ordering::Greater => {
                    // Fetch block hash from the provider.
                    let block = provider
                        .get_block_with_tx_hashes(BlockId::Number(block_to_attest))
                        .await
                        .context("Fetching block hash of block to attest")?;
                    match block {
                        MaybePendingBlockWithTxHashes::Block(block) => {
                            let attestation_window = attestation_info.attestation_window;
                            State::WaitingForAttestationWindow {
                                attestation_info,
                                attestation_params: AttestationParams {
                                    block_hash: block.block_hash,
                                    start_of_attestation_window: block_to_attest
                                        + config::MIN_ATTESTATION_WINDOW,
                                    end_of_attestation_window: block_to_attest
                                        + attestation_window as u64,
                                },
                            }
                        }
                        _ => State::WaitingForBlockToAttest {
                            attestation_info,
                            block_to_attest,
                        },
                    }
                }
            },
            State::WaitingForAttestationWindow {
                attestation_info,
                attestation_params,
            } => {
                if attestation_params.in_window(block_number) {
                    tracing::debug!(block_hash=%attestation_params.block_hash, "Sending attestation transaction");
                    let tx_hash = attest::attest(provider, signer, attestation_params.block_hash)
                        .await
                        .context("Sending attestation transaction")?;
                    tracing::debug!(tx_hash=%tx_hash, "Transaction hash");
                    State::WaitingForNextEpoch { attestation_info }
                } else {
                    State::WaitingForAttestationWindow {
                        attestation_info,
                        attestation_params,
                    }
                }
            }
            State::WaitingForNextEpoch { attestation_info } => {
                State::WaitingForNextEpoch { attestation_info }
            }
        })
    }
}

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

    let provider = JsonRpcClient::new(HttpTransport::new(Url::parse(config::NODE_URL_HTTP)?));
    let attestation_info =
        attestation_info::get_attestation_info(&provider, config::STAKER_OPERATIONAL_ADDRESS)
            .await
            .context("Getting attestation info")?;
    tracing::debug!(?attestation_info, "Current attestation info");
    let mut state = State::from_attestation_info(attestation_info)?;

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
                        tracing::info!("Received new block header: {:?}", header);
                        metrics::gauge!("validator_attestation_starknet_latest_block_number").set(header.block_number as f64);

                        // FIXME: handle reorgs
                        let new_state = state.handle_new_block_header(&provider, &signer, header.block_number, header.block_hash).await?;
                        tracing::debug!(?new_state, "State transition complete");
                        state = new_state;
                    },
                    None => tracing::warn!("Channel closed before receiving new block header"),
                }
            }
            event = events_rx.recv() => {
                match event {
                    Some(event) => tracing::info!("Received new event: {:?}", event),
                    None => tracing::warn!("Channel closed before receiving new event"),
                }
            }
        }
    }
}
