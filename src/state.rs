use std::{cmp::Ordering, time::SystemTime};

use anyhow::Context;
use starknet_crypto::Felt;

use crate::{
    attestation_info::AttestationInfo, events::AttestationEvent, signer::AttestationSigner,
};

/// Minimum attestation window.
///
/// The block hash to attest must be available at the start of the attestation
/// window. On Starknet, block hash of block N becomes available at block N +
/// 10.
const MIN_ATTESTATION_WINDOW: u64 = 10;

#[derive(Clone, Debug, PartialEq)]
pub struct AttestationParams {
    block_hash: Felt,
    start_of_attestation_window: u64,
    end_of_attestation_window: u64,
}

impl AttestationParams {
    pub fn in_window(&self, block_number: u64) -> Ordering {
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

    pub async fn handle_new_block_header<
        C: crate::jsonrpc::Client + Send + Sync + 'static,
        // S: starknet::signers::Signer + Send + Sync + 'static,
    >(
        self,
        client: &C,
        operational_address: Felt,
        signer: &AttestationSigner,
        block_number: u64,
        block_hash: Felt,
    ) -> anyhow::Result<Self> {
        // Check if a new epoch has started and re-initialize
        let state = if self.block_in_current_epoch(block_number) {
            self
        } else {
            let attestation_info = client
                .get_attestation_info(operational_address)
                .await
                .context("Getting attestation info")?;
            tracing::info!(
                staker_address=?attestation_info.staker_address,
                operational_address=?attestation_info.operational_address,
                stake=%attestation_info.stake,
                epoch_id=%attestation_info.epoch_id,
                epoch_start=%attestation_info.current_epoch_starting_block,
                epoch_length=%attestation_info.epoch_len,
                attestation_window=%attestation_info.attestation_window,
                "New epoch started"
            );
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
                            start_of_attestation_window: block_number + MIN_ATTESTATION_WINDOW,
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
                                        + MIN_ATTESTATION_WINDOW,
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
                        let result = client
                            .attest(
                                attestation_info.operational_address,
                                signer,
                                attestation_params.block_hash,
                            )
                            .await;
                        match result {
                            Ok(transaction_hash) => {
                                tracing::info!(?transaction_hash, "Attestation transaction sent");

                                metrics::gauge!(
                                    "validator_attestation_last_attestation_timestamp_seconds"
                                )
                                .set(
                                    SystemTime::now()
                                        .duration_since(SystemTime::UNIX_EPOCH)?
                                        .as_secs_f64(),
                                );
                                metrics::counter!(
                                    "validator_attestation_attestation_submitted_count"
                                )
                                .increment(1);
                            }
                            Err(err) => {
                                tracing::error!(error = ?err, "Failed to send attestation transaction");
                                metrics::counter!(
                                    "validator_attestation_attestation_failure_count"
                                )
                                .increment(1);
                            }
                        };

                        // Update operational account balance after any attestation attempt
                        // Even if RPC returns error, the transaction might have succeeded on-chain
                        if let Ok(balance) = client.get_strk_balance(attestation_info.operational_address).await {
                            let balance_strk = balance as f64 / 1e18;
                            metrics::gauge!("validator_attestation_operational_account_balance_strk").set(balance_strk);
                            tracing::debug!("Updated operational account balance after attestation attempt: {} STRK", balance_strk);
                        } else {
                            tracing::warn!("Failed to update operational account balance after attestation attempt");
                        }
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
                    metrics::counter!("validator_attestation_missed_epochs_count").increment(1);
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
        match self {
            State::Attesting {
                attestation_info,
                attestation_params,
            } => {
                if attestation_info.staker_address == staker_address
                    && attestation_info.epoch_id == epoch_id
                {
                    tracing::info!(?staker_address, %epoch_id, "Attestation confirmed");
                    metrics::counter!("validator_attestation_attestation_confirmed_count")
                        .increment(1);
                    Self::WaitingForNextEpoch { attestation_info }
                } else {
                    tracing::trace!(?staker_address, %epoch_id, "Skipping attestation successful event for other staker");
                    State::Attesting {
                        attestation_info,
                        attestation_params,
                    }
                }
            }
            _ => self,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use assert_matches::assert_matches;
    use starknet::{
        macros::felt,
        signers::{LocalWallet, SigningKey},
    };

    use crate::jsonrpc::ClientError;

    use super::*;

    #[test]
    fn test_attestation_params_in_window() {
        let attestation_params = AttestationParams {
            block_hash: Felt::ZERO,
            start_of_attestation_window: 10,
            end_of_attestation_window: 20,
        };

        assert_eq!(attestation_params.in_window(5), Ordering::Less);
        assert_eq!(attestation_params.in_window(10), Ordering::Equal);
        assert_eq!(attestation_params.in_window(15), Ordering::Equal);
        assert_eq!(attestation_params.in_window(20), Ordering::Greater);
        assert_eq!(attestation_params.in_window(25), Ordering::Greater);
    }

    const STAKER_ADDRESS: Felt = felt!("0xdeadbeef");
    const OPERATIONAL_ADDRESS: Felt = felt!("0xfeedbeef");
    const STAKE: u128 = 1000;
    const EPOCH_ID: u64 = 1;
    const BLOCK_HASH: Felt = felt!("0x123456789abcdef");
    const TRANSACTION_HASH: Felt = felt!("0xabcdef123456789");

    #[tokio::test]
    async fn test_normal_flow() {
        let initial_attestation_info = AttestationInfo {
            staker_address: STAKER_ADDRESS,
            operational_address: OPERATIONAL_ADDRESS,
            stake: STAKE,
            epoch_id: EPOCH_ID,
            current_epoch_starting_block: 0,
            epoch_len: 40,
            attestation_window: 20,
        };
        let initial_block_to_attest =
            initial_attestation_info.calculate_expected_attestation_block();

        let next_attestation_info: AttestationInfo = AttestationInfo {
            epoch_id: EPOCH_ID + 1,
            current_epoch_starting_block: initial_attestation_info.current_epoch_starting_block
                + initial_attestation_info.epoch_len,
            ..initial_attestation_info
        };
        let next_block_to_attest = next_attestation_info.calculate_expected_attestation_block();

        let client = MockClient::new(next_attestation_info.clone());
        let signer = AttestationSigner::new_local(LocalWallet::from_signing_key(
            SigningKey::from_secret_scalar(felt!("0x123456789abcdef")),
        ));
        let state = State::from_attestation_info(initial_attestation_info.clone());

        // Block before the block to attest
        let state = state
            .handle_new_block_header(&client, OPERATIONAL_ADDRESS, &signer, 0, BLOCK_HASH)
            .await
            .unwrap();
        assert_matches!(
            &state,
            State::BeforeBlockToAttest {
                block_to_attest,
                attestation_info
            } if *block_to_attest == initial_block_to_attest && *attestation_info == initial_attestation_info
        );

        // Block to attest
        let state = state
            .handle_new_block_header(
                &client,
                OPERATIONAL_ADDRESS,
                &signer,
                initial_block_to_attest,
                BLOCK_HASH,
            )
            .await
            .unwrap();
        assert_matches!(&state, State::Attesting { attestation_params, .. } if *attestation_params == AttestationParams {
            block_hash: BLOCK_HASH,
            start_of_attestation_window: initial_block_to_attest + MIN_ATTESTATION_WINDOW,
            end_of_attestation_window: initial_block_to_attest + initial_attestation_info.attestation_window as u64,
        });
        assert!(!client.attestation_sent());

        // First block within the attestation window
        let state = state
            .handle_new_block_header(
                &client,
                OPERATIONAL_ADDRESS,
                &signer,
                initial_block_to_attest + MIN_ATTESTATION_WINDOW,
                BLOCK_HASH,
            )
            .await
            .unwrap();
        assert_matches!(&state, State::Attesting { .. });
        assert!(client.attestation_sent());

        // Confirmation event for the attestation
        let state = state.handle_new_event(AttestationEvent::StakerAttestationSuccessful {
            staker_address: STAKER_ADDRESS,
            epoch_id: EPOCH_ID,
        });
        assert_matches!(state, State::WaitingForNextEpoch { .. });

        // First block of next epoch
        let state = state
            .handle_new_block_header(
                &client,
                OPERATIONAL_ADDRESS,
                &signer,
                initial_attestation_info.epoch_len,
                BLOCK_HASH,
            )
            .await
            .unwrap();
        assert_matches!(&state, State::BeforeBlockToAttest { attestation_info, block_to_attest } if *attestation_info == next_attestation_info && *block_to_attest == next_block_to_attest);

        // Block to attest in the next epoch
        let state = state
            .handle_new_block_header(
                &client,
                OPERATIONAL_ADDRESS,
                &signer,
                next_attestation_info.calculate_expected_attestation_block(),
                BLOCK_HASH,
            )
            .await
            .unwrap();
        assert_matches!(&state, State::Attesting { attestation_params, .. } if *attestation_params == AttestationParams {
            block_hash: BLOCK_HASH,
            start_of_attestation_window: next_block_to_attest + MIN_ATTESTATION_WINDOW,
            end_of_attestation_window: next_block_to_attest + next_attestation_info.attestation_window as u64,
        });
        assert!(!client.attestation_sent());
    }

    #[tokio::test]
    async fn test_starting_up_after_block_to_attest() {
        let initial_attestation_info = AttestationInfo {
            staker_address: STAKER_ADDRESS,
            operational_address: OPERATIONAL_ADDRESS,
            stake: STAKE,
            epoch_id: EPOCH_ID,
            current_epoch_starting_block: 0,
            epoch_len: 40,
            attestation_window: 20,
        };
        let initial_block_to_attest =
            initial_attestation_info.calculate_expected_attestation_block();

        let next_attestation_info = AttestationInfo {
            epoch_id: EPOCH_ID + 1,
            current_epoch_starting_block: initial_attestation_info.current_epoch_starting_block
                + initial_attestation_info.epoch_len,
            ..initial_attestation_info
        };
        let next_block_to_attest = next_attestation_info.calculate_expected_attestation_block();

        let client = MockClient::new(next_attestation_info.clone());
        let signer = AttestationSigner::new_local(LocalWallet::from_signing_key(
            SigningKey::from_secret_scalar(felt!("0x123456789abcdef")),
        ));
        let state = State::from_attestation_info(initial_attestation_info.clone());

        // Block after the block to attest
        let state = state
            .handle_new_block_header(
                &client,
                OPERATIONAL_ADDRESS,
                &signer,
                initial_block_to_attest + 1,
                BLOCK_HASH,
            )
            .await
            .unwrap();
        assert_matches!(&state, State::Attesting { attestation_params, .. } if *attestation_params == AttestationParams {
            block_hash: BLOCK_HASH,
            start_of_attestation_window: initial_block_to_attest + MIN_ATTESTATION_WINDOW,
            end_of_attestation_window: initial_block_to_attest + initial_attestation_info.attestation_window as u64,
        });
        assert!(client.block_hash_queried());
        assert!(!client.attestation_sent());

        // First block within the attestation window
        let state = state
            .handle_new_block_header(
                &client,
                OPERATIONAL_ADDRESS,
                &signer,
                initial_block_to_attest + MIN_ATTESTATION_WINDOW,
                BLOCK_HASH,
            )
            .await
            .unwrap();
        assert_matches!(&state, State::Attesting { .. });
        assert!(client.attestation_sent());

        // Confirmation event for the attestation
        let state = state.handle_new_event(AttestationEvent::StakerAttestationSuccessful {
            staker_address: STAKER_ADDRESS,
            epoch_id: EPOCH_ID,
        });
        assert_matches!(state, State::WaitingForNextEpoch { .. });

        // First block of next epoch
        let state = state
            .handle_new_block_header(
                &client,
                OPERATIONAL_ADDRESS,
                &signer,
                initial_attestation_info.epoch_len,
                BLOCK_HASH,
            )
            .await
            .unwrap();
        assert_matches!(&state, State::BeforeBlockToAttest { attestation_info, block_to_attest } if *attestation_info == next_attestation_info && *block_to_attest == next_block_to_attest);

        // Block to attest in the next epoch
        let state = state
            .handle_new_block_header(
                &client,
                OPERATIONAL_ADDRESS,
                &signer,
                next_attestation_info.calculate_expected_attestation_block(),
                BLOCK_HASH,
            )
            .await
            .unwrap();
        assert_matches!(&state, State::Attesting { attestation_params, .. } if *attestation_params == AttestationParams {
            block_hash: BLOCK_HASH,
            start_of_attestation_window: next_block_to_attest + MIN_ATTESTATION_WINDOW,
            end_of_attestation_window: next_block_to_attest + next_attestation_info.attestation_window as u64,
        });
        assert!(!client.attestation_sent());
    }

    struct MockClient {
        attestation_info: AttestationInfo,
        attestation_sent: AtomicBool,
        block_hash_queried: AtomicBool,
    }

    impl MockClient {
        fn new(attestation_info: AttestationInfo) -> Self {
            MockClient {
                attestation_info,
                attestation_sent: AtomicBool::new(false),
                block_hash_queried: AtomicBool::new(false),
            }
        }

        fn attestation_sent(&self) -> bool {
            self.attestation_sent
                .load(std::sync::atomic::Ordering::Relaxed)
        }

        fn block_hash_queried(&self) -> bool {
            self.block_hash_queried
                .load(std::sync::atomic::Ordering::Relaxed)
        }
    }

    impl crate::jsonrpc::Client for MockClient {
        async fn attest(
            &self,
            operational_address: Felt,
            _signer: &AttestationSigner,
            block_hash: Felt,
        ) -> Result<Felt, ClientError> {
            assert_eq!(operational_address, OPERATIONAL_ADDRESS);
            assert_eq!(block_hash, BLOCK_HASH);

            self.attestation_sent
                .store(true, std::sync::atomic::Ordering::Relaxed);

            Ok(TRANSACTION_HASH)
        }

        async fn attestation_done_in_current_epoch(
            &self,
            staker_address: Felt,
        ) -> Result<bool, ClientError> {
            assert_eq!(self.attestation_info.staker_address, staker_address);

            Ok(false)
        }

        async fn get_attestation_info(
            &self,
            _operational_address: Felt,
        ) -> Result<AttestationInfo, ClientError> {
            self.attestation_sent
                .store(false, std::sync::atomic::Ordering::Relaxed);

            Ok(self.attestation_info.clone())
        }

        async fn get_block_hash(&self, _block_number: u64) -> Result<Felt, ClientError> {
            self.block_hash_queried
                .store(true, std::sync::atomic::Ordering::Relaxed);

            Ok(BLOCK_HASH)
        }

        async fn get_strk_balance(&self, _account_address: Felt) -> Result<u128, ClientError> {
            // Return a mock balance of 100 STRK
            Ok(100_000_000_000_000_000_000) // 100 * 10^18
        }
    }
}
