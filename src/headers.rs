use std::time::Duration;

use anyhow::Context;
use starknet::core::types::{BlockHeader, ConfirmedBlockId, ReorgData};
use starknet_tokio_tungstenite::{NewHeadsUpdate, TungsteniteStream};
use url::Url;

pub async fn fetch(
    url: Url,
    headers_tx: tokio::sync::mpsc::Sender<BlockHeader>,
    reorg_tx: tokio::sync::mpsc::Sender<ReorgData>,
) -> anyhow::Result<()> {
    let stream = TungsteniteStream::connect(&url, Duration::from_secs(30)).await?;
    let mut subscription = stream.subscribe_new_heads(ConfirmedBlockId::Latest).await?;
    tracing::debug!("Subscription to new block headers established");

    loop {
        match subscription.recv().await {
            Ok(NewHeadsUpdate::NewHeader(header)) => {
                tracing::trace!(?header, "Received new header notification");
                headers_tx
                    .send(header)
                    .await
                    .context("Sending new block header to channel")?;
            }
            Ok(NewHeadsUpdate::Reorg(reorg)) => {
                tracing::trace!(?reorg, "Received reorg notification");
                reorg_tx
                    .send(reorg)
                    .await
                    .context("Sending reorg notification to channel")?;
            }
            Err(err) => return Err(err.into()),
        }
    }
}
