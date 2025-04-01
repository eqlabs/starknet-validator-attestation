use anyhow::Context;
use futures_util::StreamExt;
use reqwest_websocket::Message;
use url::Url;

use crate::subscription;

pub async fn fetch(
    url: Url,
    headers_tx: tokio::sync::mpsc::Sender<subscription::NewHeader>,
    reorg_tx: tokio::sync::mpsc::Sender<subscription::ReorgData>,
) -> anyhow::Result<()> {
    let (mut client, subscription_id) = subscription::subscribe(
        url.clone(),
        subscription::SubscriptionMethod::SubscribeNewHeads {},
    )
    .await?;

    tracing::debug!(
        %subscription_id,
        "Subscription to new block headers established"
    );

    loop {
        let message = client
            .next()
            .await
            .context("Receiving new block header notification")??;

        if let Message::Text(text) = message {
            let notification: subscription::SubscriptionNotification =
                serde_json::from_str(&text).context("Parsing new block header notification")?;
            match notification.method {
                subscription::NotificationMethod::NewHeads(params) => {
                    tracing::trace!(?params, "Received new header notification");
                    if params.subscription_id == subscription_id {
                        headers_tx
                            .send(params.result)
                            .await
                            .context("Sending new block header to channel")?;
                    }
                }
                subscription::NotificationMethod::Reorg(params) => {
                    tracing::trace!(?params, "Received reorg notification");
                    if params.subscription_id == subscription_id {
                        tracing::debug!(?params, "Received reorg notification");
                        reorg_tx
                            .send(params.result)
                            .await
                            .context("Sending reorg notification to channel")?;
                    }
                }
                subscription::NotificationMethod::Events(_) => {
                    tracing::warn!("Received events notification, but not handling it");
                }
            }
        }
    }
}
