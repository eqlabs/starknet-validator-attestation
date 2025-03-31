use anyhow::Context;
use futures_util::StreamExt;
use reqwest_websocket::Message;
use starknet::core::utils::get_selector_from_name;
use starknet_crypto::Felt;
use url::Url;

use crate::subscription::{self, EmittedEvent};

#[derive(Debug)]
pub enum AttestationEvent {
    StakerAttestationSuccessful { staker_address: Felt, epoch_id: u64 },
}

pub async fn fetch(
    url: Url,
    attestation_contract_address: Felt,
    tx: tokio::sync::mpsc::Sender<AttestationEvent>,
) -> anyhow::Result<()> {
    let staker_attestation_successful_selector =
        get_selector_from_name("StakerAttestationSuccessful").expect("Event name should be valid");
    let (mut client, subscription_id) = subscription::subscribe(
        url,
        subscription::SubscriptionMethod::SubscribeEvents {
            from_address: Some(attestation_contract_address),
            keys: vec![vec![staker_attestation_successful_selector]],
            block_id: None,
        },
    )
    .await?;

    tracing::debug!(
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
                    tracing::trace!(?params, "Received events notification");
                    if params.subscription_id == subscription_id {
                        let selector = params.result.keys.first().unwrap_or(&Felt::ZERO);

                        if *selector == staker_attestation_successful_selector {
                            match parse_staker_attestation_successful(&params.result) {
                                Ok(event) => tx
                                    .send(event)
                                    .await
                                    .context("Sending new event to channel")?,
                                Err(err) => tracing::debug!("Failed to parse event: {}", err),
                            }
                        } else {
                            tracing::debug!(?params, "Received unknown event");
                        }
                    }
                }
                subscription::NotificationMethod::NewHeadsNotification(_) => {
                    tracing::warn!("Received new heads notification, but not handling it");
                }
            }
        } else {
            tracing::trace!(?message, "Unexpected Websocket message");
        }
    }
}

fn parse_staker_attestation_successful(event: &EmittedEvent) -> anyhow::Result<AttestationEvent> {
    let staker_address = *event.keys.get(1).context("Getting staker address")?;
    let epoch_id = u64::try_from(*event.data.first().context("Getting epoch ID")?)
        .context("Parsing epoch ID")?;

    Ok(AttestationEvent::StakerAttestationSuccessful {
        staker_address,
        epoch_id,
    })
}
