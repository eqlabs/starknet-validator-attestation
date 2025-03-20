use anyhow::Context;
use futures_util::StreamExt;
use reqwest_websocket::Message;
use url::Url;

use crate::subscription;

pub async fn fetch(
    url: Url,
    tx: tokio::sync::mpsc::Sender<subscription::NewHeader>,
) -> anyhow::Result<()> {
    let (mut client, subscription_id) = subscription::subscribe(
        url.clone(),
        subscription::SubscriptionMethod::SubscribeNewHeads {},
    )
    .await?;

    tracing::info!(
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
                subscription::NotificationMethod::NewHeadsNotification(params) => {
                    tx.send(params.result)
                        .await
                        .context("Sending new block header to channel")?;
                }
                subscription::NotificationMethod::EventsNotification(_) => {
                    tracing::warn!("Received events notification, but not handling it");
                }
            }
        }
    }
}
