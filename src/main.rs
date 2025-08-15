use anyhow::Context;
use clap::Parser;
use jsonrpc::Client;
use starknet::{
    macros::felt,
    providers::{JsonRpcClient, Provider, jsonrpc::HttpTransport},
    signers::{LocalWallet, SigningKey},
};
use starknet_crypto::Felt;
use tokio::select;
use tracing_subscriber::EnvFilter;
use url::Url;

mod attestation_info;
mod events;
mod headers;
mod jsonrpc;
mod metrics_exporter;
mod signer;
mod state;
mod transactions;

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Config {
    #[arg(
        long,
        long_help = "The address of the staking contract.",
        value_name = "ADDRESS",
        env = "VALIDATOR_ATTESTATION_STAKING_CONTRACT_ADDRESS"
    )]
    staking_contract_address: Option<Felt>,
    #[arg(
        long,
        long_help = "The address of the attestation contract.",
        value_name = "ADDRESS",
        env = "VALIDATOR_ATTESTATION_ATTESTATION_CONTRACT_ADDRESS"
    )]
    attestation_contract_address: Option<Felt>,

    #[arg(
        long,
        long_help = "The address of the staker's operational account.",
        value_name = "ADDRESS",
        env = "VALIDATOR_ATTESTATION_STAKER_OPERATIONAL_ADDRESS"
    )]
    staker_operational_address: Felt,

    #[arg(
        long,
        long_help = "The URL of the Starknet node's JSON-RPC endpoint.",
        value_name = "URL",
        env = "VALIDATOR_ATTESTATION_STARKNET_NODE_URL"
    )]
    pub node_url: Url,

    #[arg(
        long,
        long_help = "The URL of the Starknet node's Websocket endpoint.",
        value_name = "URL",
        env = "VALIDATOR_ATTESTATION_STARKNET_NODE_WEBSOCKET_URL"
    )]
    pub node_websocket_url: Option<Url>,

    #[arg(
        long,
        long_help = "Use a local signer. The private key should be set in the environment \
                     variable VALIDATOR_ATTESTATION_OPERATIONAL_PRIVATE_KEY.",
        group = "signer"
    )]
    pub local_signer: bool,

    #[arg(
        long,
        long_help = "Use a remote signer at URL.",
        value_name = "URL",
        env = "VALIDATOR_ATTESTATION_REMOTE_SIGNER_URL",
        group = "signer"
    )]
    pub remote_signer_url: Option<Url>,

    #[arg(
        long,
        long_help = "The address to bind the metrics server to. You can scrape metrics from the \
                     '/metrics' path on this address.",
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

    // Configure rustls crypto provider.
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("rustls crypto provider setup should not fail");

    // Set up logging
    match config.log_format {
        LogFormat::Compact => {
            let format = tracing_subscriber::fmt::format().compact();
            tracing_subscriber::fmt()
                .event_format(format)
                .with_env_filter(EnvFilter::from_default_env())
                .init();
        }
        LogFormat::Json => {
            let format = tracing_subscriber::fmt::format().json();
            tracing_subscriber::fmt()
                .event_format(format)
                .with_env_filter(EnvFilter::from_default_env())
                .init();
        }
    };

    tracing::info!("Starting up");

    // Set up JSON-RPC client
    let http_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    let client = JsonRpcClient::new(HttpTransport::new_with_client(
        config.node_url.clone(),
        http_client,
    ));

    let chain_id = client.chain_id().await.context("Getting chain ID")?;
    let (staking_contract_address, attestation_contract_address) =
        contract_addresses_from_config(&config, chain_id)?;
    let strk_contract_address = strk_contract_address_from_chain_id(chain_id)?;

    let client = jsonrpc::StarknetRpcClient::new(
        client,
        staking_contract_address,
        attestation_contract_address,
        strk_contract_address,
    );

    // Initialize Prometheus metrics
    let prometheus_handle = metrics_exporter_prometheus::PrometheusBuilder::new()
        .add_global_label("network", client.chain_id_as_string().await?)
        .install_recorder()
        .context("Creating Prometheus metrics recorder")?;
    let addr: std::net::SocketAddr = config.metrics_address.parse()?;
    metrics_exporter::spawn(addr, prometheus_handle)
        .await
        .context("Staring metrics exporter")?;

    // Set up signer
    let signer = if config.local_signer {
        tracing::info!("Using local signer");
        let signer = LocalWallet::from_signing_key(SigningKey::from_secret_scalar(
            Felt::from_hex(
                &std::env::var("VALIDATOR_ATTESTATION_OPERATIONAL_PRIVATE_KEY").expect(
                    "VALIDATOR_ATTESTATION_OPERATIONAL_PRIVATE_KEY environment variable should be \
                     set to the private key",
                ),
            )
            .context("Parsing private key")?,
        ));
        signer::AttestationSigner::new_local(signer)
    } else if let Some(url) = config.remote_signer_url {
        tracing::info!(%url, "Using remote signer");
        signer::AttestationSigner::new_remote(url).context("Creating remote signer")?
    } else {
        anyhow::bail!("Either local_signer or remote_signer_url must be specified");
    };

    // Set up block and event fetchers
    let node_websocket_url = match config.node_websocket_url {
        Some(url) => url,
        None => {
            tracing::info!("Using JSON-RPC URL as WebSocket URL");
            let ws_scheme = match config.node_url.scheme() {
                "http" => "ws",
                "https" => "wss",
                _ => panic!("Unsupported Starknet node URL scheme"),
            };
            let mut node_websocket_url = config.node_url.clone();
            node_websocket_url
                .set_scheme(ws_scheme)
                .map_err(|_| anyhow::anyhow!("Failed to construct WebSocket URL"))?;
            node_websocket_url
        }
    };

    let (reorg_tx, mut reorg_rx) = tokio::sync::mpsc::channel(10);

    let (new_heads_tx, mut new_heads_rx) = tokio::sync::mpsc::channel(10);
    let mut new_block_fetcher_handle = tokio::task::spawn(headers::fetch(
        node_websocket_url.clone(),
        new_heads_tx.clone(),
        reorg_tx.clone(),
    ));

    let (events_tx, mut events_rx) = tokio::sync::mpsc::channel(10);
    let mut events_fetcher_handle = tokio::task::spawn(events::fetch(
        node_websocket_url.clone(),
        attestation_contract_address,
        events_tx.clone(),
        reorg_tx.clone(),
    ));

    let (transactions_tx, mut transactions_rx) = tokio::sync::mpsc::channel(10);
    let mut transactions_fetcher_handle = tokio::task::spawn(transactions::fetch(
        node_websocket_url.clone(),
        transactions_tx.clone(),
    ));

    // Initialize state
    let attestation_info = loop {
        match client
            .get_attestation_info(config.staker_operational_address)
            .await
            .context("Getting attestation info")
        {
            Ok(attestation_info) => {
                break attestation_info;
            }
            Err(error) => {
                tracing::info!(
                    ?error,
                    "Failed to query initial attestation info, staker not registered, retrying"
                );
                tokio::time::sleep(TASK_RESTART_DELAY).await;
            }
        }
    };
    tracing::info!(
        staker_address=?attestation_info.staker_address,
        operational_address=?attestation_info.operational_address,
        stake=%attestation_info.stake,
        epoch_id=%attestation_info.epoch_id,
        epoch_start=%attestation_info.current_epoch_starting_block,
        epoch_length=%attestation_info.epoch_len,
        attestation_window=%attestation_info.attestation_window,
        "Current attestation info"
    );
    let mut state = state::State::from_attestation_info(attestation_info);

    // Initialize operational account balance metric
    update_operational_balance(&client, config.staker_operational_address).await;

    // Handle TERM and INT signals
    let mut term_signal = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .context("Setting up TERM signal handler")?;
    let mut int_signal = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .context("Setting up INT signal handler")?;

    loop {
        select! {
            _ = term_signal.recv() => {
                tracing::info!("Received TERM signal, shutting down");
                break;
            }
            _ = int_signal.recv() => {
                tracing::info!("Received INT signal, shutting down");
                break;
            }
            block_fetcher_result = &mut new_block_fetcher_handle => {
                tracing::error!(error=?block_fetcher_result, "New block fetcher task has exited, restarting");
                let new_block_fetcher_fut = headers::fetch(node_websocket_url.clone(), new_heads_tx.clone(), reorg_tx.clone());
                new_block_fetcher_handle = tokio::task::spawn(async move {
                    tokio::time::sleep(TASK_RESTART_DELAY).await;
                    new_block_fetcher_fut.await
                });
            }
            events_fetcher_result = &mut events_fetcher_handle => {
                tracing::error!(error=?events_fetcher_result, "Events fetcher task has exited, restarting");
                let events_fetcher_fut = events::fetch(node_websocket_url.clone(), attestation_contract_address, events_tx.clone(), reorg_tx.clone());
                events_fetcher_handle = tokio::task::spawn(async move {
                    tokio::time::sleep(TASK_RESTART_DELAY).await;
                    events_fetcher_fut.await
                });
            }
            transactions_fetcher_result = &mut transactions_fetcher_handle => {
                tracing::error!(error=?transactions_fetcher_result, "Transactions fetcher task has exited, restarting");
                let transactions_fetcher_fut = transactions::fetch(node_websocket_url.clone(), transactions_tx.clone());
                transactions_fetcher_handle = tokio::task::spawn(async move {
                    tokio::time::sleep(TASK_RESTART_DELAY).await;
                    transactions_fetcher_fut.await
                });
            }
            new_block_header = new_heads_rx.recv() => {
                match new_block_header {
                    Some(header) => {
                        tracing::debug!("Received new block header: {:?}", header);
                        metrics::gauge!("validator_attestation_starknet_latest_block_number").set(header.block_number as f64);

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
            transaction = transactions_rx.recv() => {
                match transaction {
                    Some(tx) => {
                        tracing::debug!("Received new transaction: {:?}", tx);
                        // state = state.handle_new_transaction(&client, &signer, tx).await;
                        // tracing::debug!(new_state=?state, "State transition complete");
                    },
                    None => tracing::warn!("New transaction channel closed"),
                }
            }
            reorg = reorg_rx.recv() => {
                match reorg {
                    Some(reorg) => {
                        tracing::debug!(?reorg, "Received reorg notification, reinitializing");
                        if let Ok(attestation_info) = client
                            .get_attestation_info(config.staker_operational_address)
                            .await
                            .context("Getting attestation info")
                        {
                            tracing::info!(
                                staker_address=?attestation_info.staker_address,
                                operational_address=?attestation_info.operational_address,
                                stake=%attestation_info.stake,
                                epoch_id=%attestation_info.epoch_id,
                                epoch_start=%attestation_info.current_epoch_starting_block,
                                epoch_length=%attestation_info.epoch_len,
                                attestation_window=%attestation_info.attestation_window,
                                "Current attestation info"
                            );
                            state = state::State::from_attestation_info(attestation_info);
                        } else {
                            tracing::error!("Failed to get attestation info, retrying");
                            tokio::time::sleep(TASK_RESTART_DELAY).await;
                            let _ = reorg_tx.send(reorg).await.context("Re-sending reorg notification");
                        }
                    },
                    None => tracing::warn!("Reorg channel closed"),
                }
            }
        }
    }

    tracing::info!("Stopped");

    Ok(())
}

fn contract_addresses_from_config(config: &Config, chain_id: Felt) -> anyhow::Result<(Felt, Felt)> {
    const MAINNET_STAKING_CONTRACT_ADDRESS: Felt =
        felt!("0x00ca1702e64c81d9a07b86bd2c540188d92a2c73cf5cc0e508d949015e7e84a7");
    const SEPOLIA_STAKING_CONTRACT_ADDRESS: Felt =
        felt!("0x03745ab04a431fc02871a139be6b93d9260b0ff3e779ad9c8b377183b23109f1");

    let staking_contract_address = config.staking_contract_address.or_else(|| {
        if chain_id == starknet::core::chain_id::MAINNET {
            Some(MAINNET_STAKING_CONTRACT_ADDRESS)
        } else if chain_id == starknet::core::chain_id::SEPOLIA {
            Some(SEPOLIA_STAKING_CONTRACT_ADDRESS)
        } else {
            None
        }
    }).with_context(||
            format!("Staking contract address is required for chain ID {}, please specify it explicitly", chain_id),
    )?;

    const MAINNET_ATTESTATION_CONTRACT_ADDRESS: Felt =
        felt!("0x010398fe631af9ab2311840432d507bf7ef4b959ae967f1507928f5afe888a99");
    const SEPOLIA_ATTESTATION_CONTRACT_ADDRESS: Felt =
        felt!("0x3f32e152b9637c31bfcf73e434f78591067a01ba070505ff6ee195642c9acfb");

    let attestation_contract_address = config
        .attestation_contract_address
        .or_else(|| {
            if chain_id == starknet::core::chain_id::MAINNET {
                Some(MAINNET_ATTESTATION_CONTRACT_ADDRESS)
            } else if chain_id == starknet::core::chain_id::SEPOLIA {
                Some(SEPOLIA_ATTESTATION_CONTRACT_ADDRESS)
            } else {
                None
            }
        })
        .with_context(||
            format!("Attestation contract address is required for chain ID {}, please specify it explicitly", chain_id),
        )?;

    Ok((staking_contract_address, attestation_contract_address))
}

fn strk_contract_address_from_chain_id(chain_id: Felt) -> anyhow::Result<Felt> {
    // STRK contract address (same for both mainnet and testnet currently)
    const STRK_CONTRACT_ADDRESS: Felt =
        felt!("0x04718f5a0fc34cc1af16a1cdee98ffb20c31f5cd61d6ab07201858f4287c938d");

    // Support mainnet and sepolia (both use same contract address currently)
    if chain_id == starknet::core::chain_id::MAINNET
        || chain_id == starknet::core::chain_id::SEPOLIA
    {
        Ok(STRK_CONTRACT_ADDRESS)
    } else {
        anyhow::bail!(
            "STRK contract address is not configured for chain ID {}",
            chain_id
        )
    }
}

// Helper function to update operational account balance
async fn update_operational_balance<C: Client>(client: &C, operational_address: Felt) {
    match client.get_strk_balance(operational_address).await {
        Ok(balance) => {
            // Convert to floating point STRK (divide by 10^18)
            let balance_strk = balance as f64 / 1e18;
            metrics::gauge!("validator_attestation_operational_account_balance_strk")
                .set(balance_strk);
            tracing::debug!(%balance_strk, "Updated operational account balance");
        }
        Err(err) => {
            tracing::warn!(error=%err, "Failed to get operational account STRK balance");
        }
    }
}
