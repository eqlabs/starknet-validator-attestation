use anyhow::Context;
use futures_util::StreamExt;
use reqwest_websocket::Message;
use url::Url;

use crate::subscription;

pub async fn fetch(
    url: Url,
    tx: tokio::sync::mpsc::Sender<subscription::EmittedEvent>,
) -> anyhow::Result<()> {
    let (mut client, subscription_id) = subscription::subscribe(
        url.clone(),
        subscription::SubscriptionMethod::SubscribeEvents {
            from_address: None,
            keys: vec![],
            block_id: None,
        },
    )
    .await?;

    tracing::info!(
        %subscription_id,
        "Subscription to events established"
    );

    loop {
        let message = client
            .next()
            .await
            .context("Receiving new event notification")??;

        if let Message::Text(text) = message {
            let notification: subscription::SubscriptionNotification =
                serde_json::from_str(&text).context("Parsing new event notification")?;
            match notification.method {
                subscription::NotificationMethod::EventsNotification(params) => {
                    tx.send(params.result)
                        .await
                        .context("Sending new event to channel")?;
                }
                subscription::NotificationMethod::NewHeadsNotification(_) => {
                    tracing::warn!("Received new heads notification, but not handling it");
                }
            }
        }
    }
}
