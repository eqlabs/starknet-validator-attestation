use anyhow::Context;
use futures_util::StreamExt;
use reqwest_websocket::Message;
use starknet::core::utils::get_selector_from_name;
use starknet_crypto::Felt;
use url::Url;

use crate::subscription;

#[derive(Debug)]
pub enum AttestationEvent {
    StakerAttestationSuccessful { staker_address: Felt, epoch_id: u64 },
}

pub async fn fetch(
    url: Url,
    attestation_contract_address: Felt,
    event_tx: tokio::sync::mpsc::Sender<AttestationEvent>,
    reorg_tx: tokio::sync::mpsc::Sender<subscription::ReorgData>,
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
                subscription::NotificationMethod::Events(params) => {
                    tracing::trace!(?params, "Received events notification");
                    if params.subscription_id == subscription_id {
                        let selector = params.result.keys.first().unwrap_or(&Felt::ZERO);

                        if *selector == staker_attestation_successful_selector {
                            match parse_staker_attestation_successful(&params.result) {
                                Ok(event) => event_tx
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
                subscription::NotificationMethod::NewHeads(_) => {
                    tracing::warn!("Received new heads notification, but not handling it");
                }
            }
        } else {
            tracing::trace!(?message, "Unexpected Websocket message");
        }
    }
}

fn parse_staker_attestation_successful(
    event: &subscription::EmittedEvent,
) -> anyhow::Result<AttestationEvent> {
    let staker_address = *event.keys.get(1).context("Getting staker address")?;
    let epoch_id = u64::try_from(*event.data.first().context("Getting epoch ID")?)
        .context("Parsing epoch ID")?;

    Ok(AttestationEvent::StakerAttestationSuccessful {
        staker_address,
        epoch_id,
    })
}
