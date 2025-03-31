use anyhow::Context;
use clap::Parser;
use jsonrpc::Client;
use starknet::signers::{LocalWallet, SigningKey};
use starknet_crypto::Felt;
use tokio::select;
use url::Url;

mod attestation_info;
mod events;
mod headers;
mod jsonrpc;
mod metrics_exporter;
mod state;
mod subscription;

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Config {
    #[arg(
        long,
        long_help = "The address of the staking contract",
        value_name = "ADDRESS",
        env = "VALIDATOR_ATTESTATION_STAKING_CONTRACT_ADDRESS"
    )]
    staking_contract_address: Felt,
    #[arg(
        long,
        long_help = "The address of the attestation contract",
        value_name = "ADDRESS",
        env = "VALIDATOR_ATTESTATION_ATTESTATION_CONTRACT_ADDRESS"
    )]
    attestation_contract_address: Felt,

    #[arg(
        long,
        long_help = "The address of the staker's operational account",
        value_name = "ADDRESS",
        env = "VALIDATOR_ATTESTATION_STAKER_OPERATIONAL_ADDRESS"
    )]
    staker_operational_address: Felt,

    #[arg(
        long,
        long_help = "The URL of the Starknet node's JSON-RPC endpoint",
        value_name = "URL",
        env = "VALIDATOR_ATTESTATION_STARKNET_NODE_URL"
    )]
    pub node_url: Url,

    #[arg(
        long,
        long_help = "The address to bind the metrics server to. You can scrape metrics from the '/metrics' path on this address.",
        default_value = "127.0.0.1:9090",
        value_name = "IP:PORT",
        env = "VALIDATOR_ATTESTATION_METRICS_ADDRESS"
    )]
    pub metrics_address: String,

    #[arg(long, default_value = "compact", value_name = "FORMAT")]
    pub log_format: LogFormat,
}

#[derive(Clone, clap::ValueEnum)]
enum LogFormat {
    Compact,
    Json,
}

const TASK_RESTART_DELAY: std::time::Duration = std::time::Duration::from_secs(5);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::parse();

    // Set up logging
    match config.log_format {
        LogFormat::Compact => {
            let format = tracing_subscriber::fmt::format().compact();
            tracing_subscriber::fmt().event_format(format).init();
        }
        LogFormat::Json => {
            let format = tracing_subscriber::fmt::format().json();
            tracing_subscriber::fmt().event_format(format).init();
        }
    };

    tracing::info!("Starting up");

    let client = jsonrpc::StarknetRpcClient::new(
        config.node_url.clone(),
        config.staking_contract_address,
        config.attestation_contract_address,
    );

    // Initialize Prometheus metrics
    let prometheus_handle = metrics_exporter_prometheus::PrometheusBuilder::new()
        .add_global_label("network", client.chain_id().await?)
        .install_recorder()
        .context("Creating Prometheus metrics recorder")?;
    let addr: std::net::SocketAddr = config.metrics_address.parse()?;
    metrics_exporter::spawn(addr, prometheus_handle)
        .await
        .context("Staring metrics exporter")?;

    // Set up signer
    let signer = LocalWallet::from_signing_key(SigningKey::from_secret_scalar(
        Felt::from_hex(
            &std::env::var("VALIDATOR_ATTESTATION_OPERATIONAL_PRIVATE_KEY").expect(
                "VALIDATOR_ATTESTATION_OPERATIONAL_PRIVATE_KEY environment variable should be set to the private key",
            ),
        )
        .unwrap(),
    ));

    // Set up block and event fetchers
    let ws_scheme = match config.node_url.scheme() {
        "http" => "ws",
        "https" => "wss",
        _ => panic!("Unsupported Starknet node URL scheme"),
    };
    let mut node_websocket_url = config.node_url.clone();
    node_websocket_url
        .set_scheme(ws_scheme)
        .map_err(|_| anyhow::anyhow!("Failed to construct WebSocket URL"))?;

    let (new_heads_tx, mut new_heads_rx) = tokio::sync::mpsc::channel(10);
    let mut new_block_fetcher_handle = tokio::task::spawn(headers::fetch(
        node_websocket_url.clone(),
        new_heads_tx.clone(),
    ));

    let (events_tx, mut events_rx) = tokio::sync::mpsc::channel(10);
    let mut events_fetcher_handle = tokio::task::spawn(events::fetch(
        node_websocket_url.clone(),
        config.attestation_contract_address,
        events_tx.clone(),
    ));

    // Initialize state
    let attestation_info = client
        .get_attestation_info(config.staker_operational_address)
        .await
        .context("Getting attestation info")?;
    tracing::debug!(?attestation_info, "Current attestation info");
    let mut state = state::State::from_attestation_info(attestation_info);

    loop {
        select! {
            block_fetcher_result = &mut new_block_fetcher_handle => {
                tracing::error!(error=?block_fetcher_result, "New block fetcher task has exited, restarting");
                let new_block_fetcher_fut = headers::fetch(node_websocket_url.clone(), new_heads_tx.clone());
                new_block_fetcher_handle = tokio::task::spawn(async move {
                    tokio::time::sleep(TASK_RESTART_DELAY).await;
                    new_block_fetcher_fut.await
                });
            }
            events_fetcher_result = &mut events_fetcher_handle => {
                tracing::error!(error=?events_fetcher_result, "Events fetcher task has exited, restarting");
                let events_fetcher_fut = events::fetch(node_websocket_url.clone(), config.attestation_contract_address, events_tx.clone());
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
                        let result = state.handle_new_block_header(&client, config.staker_operational_address, &signer, header.block_number, header.block_hash).await;
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
