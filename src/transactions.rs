use std::time::Duration;

use anyhow::Context;
use starknet::core::types::TransactionWithL2Status;
use starknet_tokio_tungstenite::TungsteniteStream;
use url::Url;

pub async fn fetch(
    url: Url,
    transactions_tx: tokio::sync::mpsc::Sender<TransactionWithL2Status>,
) -> anyhow::Result<()> {
    let stream = TungsteniteStream::connect(&url, Duration::from_secs(30)).await?;
    let mut subscription = stream.subscribe_new_transactions(None, None).await?;
    tracing::debug!("Subscription to new block headers established");

    loop {
        match subscription.recv().await {
            Ok(tx) => {
                tracing::trace!(?tx, "Received new transaction notification");
                transactions_tx
                    .send(tx)
                    .await
                    .context("Sending new transaction to channel")?;
            }
            Err(err) => return Err(err.into()),
        }
    }
}
