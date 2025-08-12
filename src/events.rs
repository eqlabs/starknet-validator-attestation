use std::time::Duration;

use anyhow::Context;
use starknet::core::{
    codec::Decode,
    types::{EmittedEvent, ReorgData},
};
use starknet::macros::selector;
use starknet_crypto::Felt;
use starknet_tokio_tungstenite::{EventSubscriptionOptions, EventsUpdate, TungsteniteStream};
use url::Url;

const SELECTOR_STAKER_ATTESTATION_SUCCESSFUL: Felt = selector!("StakerAttestationSuccessful");

#[derive(Debug)]
pub enum AttestationEvent {
    StakerAttestationSuccessful { staker_address: Felt, epoch_id: u64 },
}

#[derive(Decode)]
struct StakerAttestationSuccessfulData {
    epoch: u64,
}

pub async fn fetch(
    url: Url,
    attestation_contract_address: Felt,
    event_tx: tokio::sync::mpsc::Sender<AttestationEvent>,
    reorg_tx: tokio::sync::mpsc::Sender<ReorgData>,
) -> anyhow::Result<()> {
    let stream = TungsteniteStream::connect(&url, Duration::from_secs(30)).await?;
    let mut subscription = stream
        .subscribe_events(
            EventSubscriptionOptions::new()
                .with_from_address(attestation_contract_address)
                .with_keys(vec![vec![SELECTOR_STAKER_ATTESTATION_SUCCESSFUL]]),
        )
        .await?;
    tracing::debug!("Subscription to events established");

    loop {
        match subscription.recv().await {
            Ok(EventsUpdate::Event(event)) => {
                tracing::trace!(?event, "Received events notification");

                let selector = event.emitted_event.keys.first().unwrap_or(&Felt::ZERO);
                if *selector == SELECTOR_STAKER_ATTESTATION_SUCCESSFUL {
                    match parse_staker_attestation_successful(&event.emitted_event) {
                        Ok(event) => event_tx
                            .send(event)
                            .await
                            .context("Sending new event to channel")?,
                        Err(err) => tracing::debug!("Failed to parse event: {}", err),
                    }
                } else {
                    tracing::debug!(?event, "Received unknown event");
                }
            }
            Ok(EventsUpdate::Reorg(reorg)) => {
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

fn parse_staker_attestation_successful(event: &EmittedEvent) -> anyhow::Result<AttestationEvent> {
    let staker_address = *event.keys.get(1).context("Getting staker address")?;
    let event_data =
        StakerAttestationSuccessfulData::decode(&event.data).context("Parsing event data")?;

    Ok(AttestationEvent::StakerAttestationSuccessful {
        staker_address,
        epoch_id: event_data.epoch,
    })
}
