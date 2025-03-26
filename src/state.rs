use std::{cmp::Ordering, time::SystemTime};

use anyhow::Context;
use starknet::signers::LocalWallet;
use starknet_crypto::Felt;

use crate::{attestation_info::AttestationInfo, events::AttestationEvent};

#[derive(Clone, Debug)]
pub struct AttestationParams {
    block_hash: Felt,
    start_of_attestation_window: u64,
    end_of_attestation_window: u64,
}

impl AttestationParams {
    pub fn in_window(&self, block_number: u64) -> std::cmp::Ordering {
        use std::cmp::Ordering;

        if block_number < self.start_of_attestation_window {
            Ordering::Less
        } else if block_number >= self.end_of_attestation_window {
            Ordering::Greater
        } else {
            Ordering::Equal
        }
    }
}

#[derive(Clone, Debug)]
pub enum State {
    BeforeBlockToAttest {
        attestation_info: AttestationInfo,
        block_to_attest: u64,
    },
    Attesting {
        attestation_info: AttestationInfo,
        attestation_params: AttestationParams,
    },
    WaitingForNextEpoch {
        attestation_info: AttestationInfo,
    },
}

impl State {
    pub fn from_attestation_info(attestation_info: AttestationInfo) -> Self {
        let block_to_attest = attestation_info.calculate_expected_attestation_block();

        metrics::gauge!("validator_attestation_current_epoch_id")
            .set(attestation_info.epoch_id as f64);
        metrics::gauge!("validator_attestation_current_epoch_starting_block_number")
            .set(attestation_info.current_epoch_starting_block as f64);
        metrics::gauge!("validator_attestation_current_epoch_length")
            .set(attestation_info.epoch_len as f64);
        metrics::gauge!("validator_attestation_current_epoch_assigned_block_number")
            .set(block_to_attest as f64);

        State::BeforeBlockToAttest {
            attestation_info,
            block_to_attest,
        }
    }

    fn attestation_info(&self) -> &AttestationInfo {
        match self {
            State::BeforeBlockToAttest {
                attestation_info, ..
            } => attestation_info,
            State::Attesting {
                attestation_info, ..
            } => attestation_info,
            State::WaitingForNextEpoch { attestation_info } => attestation_info,
        }
    }

    fn block_in_current_epoch(&self, block_number: u64) -> bool {
        let attestation_info = self.attestation_info();
        block_number >= attestation_info.current_epoch_starting_block
            && block_number
                < attestation_info.current_epoch_starting_block + attestation_info.epoch_len
    }

    pub async fn handle_new_block_header<C: crate::jsonrpc::Client>(
        self,
        client: &C,
        signer: &LocalWallet,
        block_number: u64,
        block_hash: Felt,
    ) -> anyhow::Result<Self> {
        // Check if a new epoch has started and re-initialize
        let state = if self.block_in_current_epoch(block_number) {
            self
        } else {
            let attestation_info = client
                .get_attestation_info(crate::config::STAKER_OPERATIONAL_ADDRESS)
                .await
                .context("Getting attestation info")?;
            tracing::info!(?attestation_info, "New epoch started");
            State::from_attestation_info(attestation_info)
        };

        Ok(match state {
            State::BeforeBlockToAttest {
                attestation_info,
                block_to_attest,
            } => match block_number.cmp(&block_to_attest) {
                // Not there yet.
                Ordering::Less => State::BeforeBlockToAttest {
                    attestation_info,
                    block_to_attest,
                },
                // We have received the block hash for the block to attest.
                Ordering::Equal => {
                    let attestation_window = attestation_info.attestation_window;
                    State::Attesting {
                        attestation_info,
                        attestation_params: AttestationParams {
                            block_hash,
                            start_of_attestation_window: block_number
                                + crate::config::MIN_ATTESTATION_WINDOW,
                            end_of_attestation_window: block_number + attestation_window as u64,
                        },
                    }
                }
                // We're past the block on the block header subscription.
                Ordering::Greater => {
                    // Fetch block hash from the provider.
                    client
                        .get_block_hash(block_to_attest)
                        .await
                        .map(|block_hash| {
                            let attestation_window = attestation_info.attestation_window;
                            State::Attesting {
                                attestation_info,
                                attestation_params: AttestationParams {
                                    block_hash,
                                    start_of_attestation_window: block_to_attest
                                        + crate::config::MIN_ATTESTATION_WINDOW,
                                    end_of_attestation_window: block_to_attest
                                        + attestation_window as u64,
                                },
                            }
                        })?
                }
            },
            State::Attesting {
                attestation_info,
                attestation_params,
            } => match attestation_params.in_window(block_number) {
                Ordering::Less => State::Attesting {
                    attestation_info,
                    attestation_params,
                },
                Ordering::Equal => {
                    let attestation_done = client
                        .attestation_done_in_current_epoch(attestation_info.staker_address)
                        .await
                        .context("Checking attestation status")?;

                    if !attestation_done {
                        tracing::debug!(block_hash=%attestation_params.block_hash, "Sending attestation transaction");
                        let result = client.attest(signer, attestation_params.block_hash).await;
                        match result {
                            Ok(transaction_hash) => {
                                tracing::info!(?transaction_hash, "Sent attestation transaction");

                                metrics::gauge!(
                                    "validator_attestation_last_attestation_timestamp_seconds"
                                )
                                .set(
                                    SystemTime::now()
                                        .duration_since(SystemTime::UNIX_EPOCH)?
                                        .as_secs_f64(),
                                );
                            }
                            Err(err) => {
                                tracing::error!(error = %err, "Failed to send attestation transaction");
                            }
                        };
                        State::Attesting {
                            attestation_info,
                            attestation_params,
                        }
                    } else {
                        tracing::debug!("Attestation already done");
                        State::WaitingForNextEpoch { attestation_info }
                    }
                }
                Ordering::Greater => {
                    // We're past the attestation window
                    State::WaitingForNextEpoch { attestation_info }
                }
            },
            State::WaitingForNextEpoch { attestation_info } => {
                State::WaitingForNextEpoch { attestation_info }
            }
        })
    }

    pub fn handle_new_event(self, event: AttestationEvent) -> Self {
        match event {
            AttestationEvent::StakerAttestationSuccessful {
                staker_address,
                epoch_id,
            } => self.handle_attestation_successful_event(staker_address, epoch_id),
        }
    }

    fn handle_attestation_successful_event(self, staker_address: Felt, epoch_id: u64) -> Self {
        let attestation_info = self.attestation_info();
        if attestation_info.staker_address == staker_address
            && attestation_info.epoch_id == epoch_id
        {
            tracing::info!(?staker_address, %epoch_id, "Attestation confirmed");
            Self::WaitingForNextEpoch {
                attestation_info: attestation_info.clone(),
            }
        } else {
            tracing::trace!(?staker_address, %epoch_id, "Skipping attestation successful event for other staker");
            self
        }
    }
}
