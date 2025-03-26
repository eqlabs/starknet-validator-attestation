use std::cmp::Ordering;

use anyhow::Context;
use starknet::signers::LocalWallet;
use starknet_crypto::Felt;

use crate::attestation_info::AttestationInfo;

#[derive(Debug)]
pub struct AttestationParams {
    block_hash: Felt,
    start_of_attestation_window: u64,
    end_of_attestation_window: u64,
}

impl AttestationParams {
    pub fn in_window(&self, block_number: u64) -> bool {
        block_number >= self.start_of_attestation_window
            && block_number < self.end_of_attestation_window
    }
}

#[derive(Debug)]
pub enum State {
    WaitingForBlockToAttest {
        attestation_info: AttestationInfo,
        block_to_attest: u64,
    },
    WaitingForAttestationWindow {
        attestation_info: AttestationInfo,
        attestation_params: AttestationParams,
    },
    WaitingForNextEpoch {
        attestation_info: AttestationInfo,
    },
}

impl State {
    pub fn from_attestation_info(attestation_info: AttestationInfo) -> anyhow::Result<Self> {
        let block_to_attest = attestation_info
            .calculate_expected_attestation_block()
            .context("Calculating expected attestation block")?;
        Ok(State::WaitingForBlockToAttest {
            attestation_info,
            block_to_attest,
        })
    }

    fn attestation_info(&self) -> &AttestationInfo {
        match self {
            State::WaitingForBlockToAttest {
                attestation_info, ..
            } => attestation_info,
            State::WaitingForAttestationWindow {
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
            tracing::debug!(?attestation_info, "New epoch started");
            State::from_attestation_info(attestation_info)?
        };

        Ok(match state {
            State::WaitingForBlockToAttest {
                attestation_info,
                block_to_attest,
            } => match block_number.cmp(&block_to_attest) {
                // Not there yet.
                Ordering::Less => State::WaitingForBlockToAttest {
                    attestation_info,
                    block_to_attest,
                },
                // We have received the block hash for the block to attest.
                Ordering::Equal => {
                    let attestation_window = attestation_info.attestation_window;
                    State::WaitingForAttestationWindow {
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
                            State::WaitingForAttestationWindow {
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
            State::WaitingForAttestationWindow {
                attestation_info,
                attestation_params,
            } => {
                if attestation_params.in_window(block_number) {
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
                                State::WaitingForNextEpoch { attestation_info }
                            }
                            Err(err) => {
                                tracing::error!(error = %err, "Failed to send attestation transaction");
                                State::WaitingForAttestationWindow {
                                    attestation_info,
                                    attestation_params,
                                }
                            }
                        }
                    } else {
                        tracing::debug!("Attestation already done");
                        State::WaitingForNextEpoch { attestation_info }
                    }
                } else {
                    State::WaitingForAttestationWindow {
                        attestation_info,
                        attestation_params,
                    }
                }
            }
            State::WaitingForNextEpoch { attestation_info } => {
                State::WaitingForNextEpoch { attestation_info }
            }
        })
    }
}
