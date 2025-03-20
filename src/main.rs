// use starknet::{
//     accounts::{Account, ExecutionEncoding, SingleOwnerAccount},
//     core::{
//         chain_id,
//         types::{BlockId, BlockTag, Call, Felt},
//         utils::get_selector_from_name,
//     },
//     providers::jsonrpc::{HttpTransport, JsonRpcClient},
//     signers::{LocalWallet, SigningKey},
// };
use anyhow::Context;
use tokio::select;
use tracing_subscriber::prelude::*;
use url::Url;

mod events;
mod headers;
mod metrics_exporter;
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
    let addr: std::net::SocketAddr = "127.0.0.1:8080".parse()?;
    metrics_exporter::spawn(addr, prometheus_handle)
        .await
        .context("Staring metrics exporter")?;

    let url = Url::parse("ws://127.0.0.1:9545/rpc/v0_8").unwrap();

    let (new_heads_tx, mut new_heads_rx) = tokio::sync::mpsc::channel(10);
    let mut new_block_fetcher_handle =
        tokio::task::spawn(headers::fetch(url.clone(), new_heads_tx.clone()));

    let (events_tx, mut events_rx) = tokio::sync::mpsc::channel(10);
    let mut events_fetcher_handle =
        tokio::task::spawn(events::fetch(url.clone(), events_tx.clone()));

    loop {
        select! {
            block_fetcher_result = &mut new_block_fetcher_handle => {
                tracing::error!(error=?block_fetcher_result, "New block fetcher task has exited, restarting");
                let new_block_fetcher_fut = headers::fetch(url.clone(), new_heads_tx.clone());
                new_block_fetcher_handle = tokio::task::spawn(async move {
                    tokio::time::sleep(TASK_RESTART_DELAY).await;
                    new_block_fetcher_fut.await
                });
            }
            events_fetcher_result = &mut events_fetcher_handle => {
                tracing::error!(error=?events_fetcher_result, "Events fetcher task has exited, restarting");
                let events_fetcher_fut = events::fetch(url.clone(), events_tx.clone());
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

    // let provider = JsonRpcClient::new(HttpTransport::new(
    //     Url::parse("https://starknet-sepolia.public.blastapi.io/rpc/v0_8").unwrap(),
    // ));

    // let signer = LocalWallet::from(SigningKey::from_secret_scalar(
    //     Felt::from_hex("YOUR_PRIVATE_KEY_IN_HEX_HERE").unwrap(),
    // ));
    // let address = Felt::from_hex("YOUR_ACCOUNT_CONTRACT_ADDRESS_IN_HEX_HERE").unwrap();
    // let tst_token_address =
    //     Felt::from_hex("07394cbe418daa16e42b87ba67372d4ab4a5df0b05c6e554d158458ce245bc10").unwrap();

    // let mut account = SingleOwnerAccount::new(
    //     provider,
    //     signer,
    //     address,
    //     chain_id::SEPOLIA,
    //     ExecutionEncoding::New,
    // );

    // // `SingleOwnerAccount` defaults to checking nonce and estimating fees against the latest
    // // block. Optionally change the target block to pending with the following line:
    // account.set_block_id(BlockId::Tag(BlockTag::Pending));

    // let result = account
    //     .execute_v3(vec![Call {
    //         to: tst_token_address,
    //         selector: get_selector_from_name("mint").unwrap(),
    //         calldata: vec![
    //             address,
    //             Felt::from_dec_str("1000000000000000000000").unwrap(),
    //             Felt::ZERO,
    //         ],
    //     }])
    //     .send()
    //     .await
    //     .unwrap();

    // println!("Transaction hash: {:#064x}", result.transaction_hash);
}
