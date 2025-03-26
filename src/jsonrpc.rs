use anyhow::Context;
use starknet::{
    accounts::{Account, AccountError, SingleOwnerAccount},
    core::{
        types::{
            BlockId, BlockTag, ContractExecutionError, FunctionCall, InnerContractExecutionError,
            MaybePendingBlockWithTxHashes,
        },
        utils::get_selector_from_name,
    },
    providers::{JsonRpcClient, Provider, ProviderError, jsonrpc::HttpTransport},
    signers::LocalWallet,
};
use starknet_crypto::Felt;
use url::Url;

use crate::attestation_info::AttestationInfo;

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("Attestation failed: {0}")]
    AttestationFailed(String),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl From<ProviderError> for ClientError {
    fn from(error: ProviderError) -> Self {
        match error {
            ProviderError::StarknetError(error) => match error {
                starknet::core::types::StarknetError::TransactionExecutionError(data) => {
                    let message = contract_execution_error_message(&data.execution_error);
                    ClientError::AttestationFailed(format!("Transaction rejected: {message}"))
                }
                _ => ClientError::AttestationFailed(error.to_string()),
            },
            _ => ClientError::Other(error.into()),
        }
    }
}

fn contract_execution_error_message(error: &ContractExecutionError) -> String {
    match error {
        ContractExecutionError::Nested(InnerContractExecutionError { error, .. }) => {
            contract_execution_error_message(error)
        }
        ContractExecutionError::Message(message) => message.clone(),
    }
}

impl<E: std::error::Error + Send + Sync + 'static> From<AccountError<E>> for ClientError {
    fn from(error: AccountError<E>) -> Self {
        match error {
            AccountError::Provider(e) => e.into(),
            _ => ClientError::Other(anyhow::anyhow!(error)),
        }
    }
}

pub trait Client {
    async fn attest(&self, signer: &LocalWallet, block_hash: Felt) -> Result<Felt, ClientError>;
    async fn attestation_done_in_current_epoch(
        &self,
        staker_address: Felt,
    ) -> Result<bool, ClientError>;
    async fn get_attestation_info(
        &self,
        operational_address: Felt,
    ) -> Result<AttestationInfo, ClientError>;
    async fn get_block_hash(&self, block_number: u64) -> Result<Felt, ClientError>;
}

pub struct StarknetRpcClient {
    client: JsonRpcClient<HttpTransport>,
}

impl Client for StarknetRpcClient {
    async fn attest(&self, signer: &LocalWallet, block_hash: Felt) -> Result<Felt, ClientError> {
        let mut account = SingleOwnerAccount::new(
            &self.client,
            signer,
            crate::config::STAKER_OPERATIONAL_ADDRESS,
            starknet::core::chain_id::SEPOLIA,
            starknet::accounts::ExecutionEncoding::New,
        );

        account.set_block_id(BlockId::Tag(BlockTag::Pending));

        let result = account
            .execute_v3(vec![starknet::core::types::Call {
                to: crate::config::ATTESTATION_CONTRACT_ADDRESS,
                selector: get_selector_from_name("attest").unwrap(),
                calldata: vec![block_hash],
            }])
            .send()
            .await?;

        Ok(result.transaction_hash)
    }

    async fn attestation_done_in_current_epoch(
        &self,
        staker_address: Felt,
    ) -> Result<bool, ClientError> {
        let result = self
            .client
            .call(
                FunctionCall {
                    contract_address: crate::config::ATTESTATION_CONTRACT_ADDRESS,
                    entry_point_selector: get_selector_from_name(
                        "is_attestation_done_in_curr_epoch",
                    )
                    .unwrap(),
                    calldata: vec![staker_address],
                },
                BlockId::Tag(BlockTag::Pending),
            )
            .await
            .unwrap();

        Ok(result == vec![Felt::ONE])
    }

    async fn get_attestation_info(
        &self,
        operational_address: Felt,
    ) -> Result<AttestationInfo, ClientError> {
        let attestation_info = self
            .client
            .call(
                FunctionCall {
                    contract_address: crate::config::STAKING_CONTRACT_ADDRESS,
                    entry_point_selector: get_selector_from_name(
                        "get_attestation_info_by_operational_address",
                    )
                    .unwrap(),
                    calldata: vec![operational_address],
                },
                BlockId::Tag(BlockTag::Pending),
            )
            .await
            .unwrap();

        let attestation_window = self
            .get_attestation_window()
            .await
            .context("Getting attestation window")?;

        Ok(AttestationInfo {
            staker_address: attestation_info[0],
            stake: attestation_info[1].try_into().context("Converting stake")?,
            epoch_len: attestation_info[2]
                .try_into()
                .context("Converting epoch length")?,
            epoch_id: attestation_info[3]
                .try_into()
                .context("Converting epoch id")?,
            current_epoch_starting_block: attestation_info[4]
                .try_into()
                .context("Converting current epoch starting block")?,
            attestation_window,
        })
    }

    async fn get_block_hash(&self, block_number: u64) -> Result<Felt, ClientError> {
        let block = self
            .client
            .get_block_with_tx_hashes(BlockId::Number(block_number))
            .await
            .context("Fetching block hash of block to attest")?;

        match block {
            MaybePendingBlockWithTxHashes::Block(block) => Ok(block.block_hash),
            MaybePendingBlockWithTxHashes::PendingBlock(_) => {
                Err(anyhow::anyhow!("Received pending block in response").into())
            }
        }
    }
}

impl StarknetRpcClient {
    pub fn new(url: Url) -> Self {
        StarknetRpcClient {
            client: JsonRpcClient::new(HttpTransport::new(url)),
        }
    }

    async fn get_attestation_window(&self) -> anyhow::Result<u16> {
        let result = self
            .client
            .call(
                FunctionCall {
                    contract_address: crate::config::ATTESTATION_CONTRACT_ADDRESS,
                    entry_point_selector: get_selector_from_name("attestation_window").unwrap(),
                    calldata: vec![],
                },
                BlockId::Tag(BlockTag::Pending),
            )
            .await
            .unwrap();

        let attestation_window = result[0]
            .try_into()
            .context("Converting attestation window")?;
        Ok(attestation_window)
    }
}
