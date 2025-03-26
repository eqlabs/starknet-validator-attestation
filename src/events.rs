use anyhow::Context;
use futures_util::StreamExt;
use reqwest_websocket::Message;
use starknet::core::utils::get_selector_from_name;
use url::Url;

use crate::subscription;

pub async fn fetch(
    url: Url,
    tx: tokio::sync::mpsc::Sender<subscription::EmittedEvent>,
) -> anyhow::Result<()> {
    let (mut client, subscription_id) = subscription::subscribe(
        url,
        subscription::SubscriptionMethod::SubscribeEvents {
            from_address: Some(crate::config::ATTESTATION_CONTRACT_ADDRESS),
            keys: vec![vec![
                get_selector_from_name("StakerAttestationSuccessful")
                    .expect("Event name should be valid"),
            ]],
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
                    if params.subscription_id == subscription_id {
                        tx.send(params.result)
                            .await
                            .context("Sending new event to channel")?;
                    }
                }
                subscription::NotificationMethod::NewHeadsNotification(_) => {
                    tracing::warn!("Received new heads notification, but not handling it");
                }
            }
        }
    }
}
