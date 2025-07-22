use anyhow::Context;
use starknet::{
    accounts::{Account, AccountError},
    core::{
        types::{
            BlockId, BlockTag, BroadcastedInvokeTransactionV3, ContractExecutionError,
            DataAvailabilityMode, FunctionCall, InnerContractExecutionError,
            MaybePendingBlockWithTxHashes, ResourceBounds, ResourceBoundsMapping,
            TransactionStatus,
        },
        utils::get_selector_from_name,
    },
    providers::{JsonRpcClient, Provider, ProviderError, jsonrpc::HttpTransport},
};
use starknet_crypto::Felt;

use crate::{
    attestation_info::AttestationInfo,
    signer::{AttestationSigner, SignError},
};

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
                _ => ClientError::AttestationFailed(format!("Starknet error: {error:?}")),
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
    async fn attest(
        &self,
        operational_address: Felt,
        signer: &AttestationSigner,
        block_hash: Felt,
    ) -> Result<Felt, ClientError>;
    async fn attestation_done_in_current_epoch(
        &self,
        staker_address: Felt,
    ) -> Result<bool, ClientError>;
    async fn attestation_status(
        &self,
        transaction_hash: Felt,
    ) -> Result<TransactionStatus, ClientError>;
    async fn get_attestation_info(
        &self,
        operational_address: Felt,
    ) -> Result<AttestationInfo, ClientError>;
    async fn get_block_hash(&self, block_number: u64) -> Result<Felt, ClientError>;
    async fn get_strk_balance(&self, account_address: Felt) -> Result<u128, ClientError>;
}

pub struct StarknetRpcClient {
    client: JsonRpcClient<HttpTransport>,
    staking_contract_address: Felt,
    attestation_contract_address: Felt,
    strk_contract_address: Felt,
}

impl Client for StarknetRpcClient {
    async fn attest(
        &self,
        operational_address: Felt,
        signer: &AttestationSigner,
        block_hash: Felt,
    ) -> Result<Felt, ClientError> {
        let chain_id = self.client.chain_id().await.context("Getting chain ID")?;

        let account = ClearSigningAccount::new(&self.client, signer, operational_address, chain_id);

        let result = account
            .execute_v3(vec![starknet::core::types::Call {
                to: self.attestation_contract_address,
                selector: get_selector_from_name("attest").unwrap(),
                calldata: vec![block_hash],
            }])
            .gas_price_estimate_multiplier(3.0)
            .gas_estimate_multiplier(3.0)
            .send()
            .await
            .context("Sending transaction")?;

        Ok(result.transaction_hash)
    }

    async fn attestation_status(
        &self,
        transaction_hash: Felt,
    ) -> Result<TransactionStatus, ClientError> {
        self.client
            .get_transaction_status(transaction_hash)
            .await
            .context("Fetching transaction status")
            .map_err(ClientError::from)
    }

    async fn attestation_done_in_current_epoch(
        &self,
        staker_address: Felt,
    ) -> Result<bool, ClientError> {
        let result = self
            .client
            .call(
                FunctionCall {
                    contract_address: self.attestation_contract_address,
                    entry_point_selector: get_selector_from_name(
                        "is_attestation_done_in_curr_epoch",
                    )
                    .unwrap(),
                    calldata: vec![staker_address],
                },
                BlockId::Tag(BlockTag::Pending),
            )
            .await?;

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
                    contract_address: self.staking_contract_address,
                    entry_point_selector: get_selector_from_name(
                        "get_attestation_info_by_operational_address",
                    )
                    .unwrap(),
                    calldata: vec![operational_address],
                },
                BlockId::Tag(BlockTag::Pending),
            )
            .await?;

        let attestation_window = self
            .get_attestation_window()
            .await
            .context("Getting attestation window")?;

        Ok(AttestationInfo {
            staker_address: attestation_info[0],
            operational_address,
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

    async fn get_strk_balance(&self, account_address: Felt) -> Result<u128, ClientError> {
        let result = self
            .client
            .call(
                FunctionCall {
                    contract_address: self.strk_contract_address,
                    entry_point_selector: get_selector_from_name("balance_of")
                        .context("Getting balance_of selector")?,
                    calldata: vec![account_address],
                },
                BlockId::Tag(BlockTag::Pending),
            )
            .await?;

        let balance: u128 = result[0].try_into().context("Converting STRK balance")?;
        Ok(balance)
    }
}

impl StarknetRpcClient {
    pub fn new(
        client: JsonRpcClient<HttpTransport>,
        staking_contract_address: Felt,
        attestation_contract_address: Felt,
        strk_contract_address: Felt,
    ) -> Self {
        StarknetRpcClient {
            client,
            staking_contract_address,
            attestation_contract_address,
            strk_contract_address,
        }
    }

    async fn get_attestation_window(&self) -> anyhow::Result<u16> {
        let result = self
            .client
            .call(
                FunctionCall {
                    contract_address: self.attestation_contract_address,
                    entry_point_selector: get_selector_from_name("attestation_window")?,
                    calldata: vec![],
                },
                BlockId::Tag(BlockTag::Pending),
            )
            .await?;

        let attestation_window = result[0]
            .try_into()
            .context("Converting attestation window")?;
        Ok(attestation_window)
    }

    pub async fn chain_id_as_string(&self) -> Result<String, ClientError> {
        let chain_id = self.client.chain_id().await.context("Getting chain ID")?;
        let chain_id = starknet::core::utils::parse_cairo_short_string(&chain_id)
            .context("Parsing chain ID as Cairo short string")?;
        Ok(chain_id)
    }
}

#[derive(Debug, Clone)]
struct ClearSigningAccount<'a, P: Provider + Send> {
    provider: P,
    signer: &'a AttestationSigner,
    address: Felt,
    chain_id: Felt,
    block_id: BlockId,
}

impl<'a, P: Provider + Send + Sync> ClearSigningAccount<'a, P> {
    pub fn new(provider: P, signer: &'a AttestationSigner, address: Felt, chain_id: Felt) -> Self {
        Self {
            provider,
            signer,
            address,
            chain_id,
            block_id: BlockId::Tag(BlockTag::Pending),
        }
    }
}

#[async_trait::async_trait]
impl<P> Account for ClearSigningAccount<'_, P>
where
    P: Provider + Sync + Send,
{
    type SignError = SignError;

    fn address(&self) -> Felt {
        self.address
    }

    fn chain_id(&self) -> Felt {
        self.chain_id
    }

    async fn sign_execution_v3(
        &self,
        execution: &starknet::accounts::RawExecutionV3,
        query_only: bool,
    ) -> Result<Vec<Felt>, Self::SignError> {
        let tx_hash = execution.transaction_hash(self.chain_id, self.address, query_only, self);
        let transaction = self.get_invoke_request(execution, query_only);

        let signature = self
            .signer
            .sign(&tx_hash, transaction, self.chain_id)
            .await?;

        Ok(signature)
    }

    async fn sign_declaration_v3(
        &self,
        _declaration: &starknet::accounts::RawDeclarationV3,
        _query_only: bool,
    ) -> Result<Vec<Felt>, Self::SignError> {
        unimplemented!("Signing declaration is not implemented. This is an internal error.");
    }

    fn is_signer_interactive(
        &self,
        context: starknet::signers::SignerInteractivityContext<'_>,
    ) -> bool {
        self.signer.is_signer_interactive(context)
    }
}

impl<P> starknet::accounts::ExecutionEncoder for ClearSigningAccount<'_, P>
where
    P: Provider + Send,
{
    fn encode_calls(&self, calls: &[starknet::core::types::Call]) -> Vec<Felt> {
        let mut execute_calldata: Vec<Felt> = vec![calls.len().into()];

        for call in calls {
            execute_calldata.push(call.to); // to
            execute_calldata.push(call.selector); // selector

            execute_calldata.push(call.calldata.len().into()); // calldata.len()
            execute_calldata.extend_from_slice(&call.calldata);
        }

        execute_calldata
    }
}

impl<P> starknet::accounts::ConnectedAccount for ClearSigningAccount<'_, P>
where
    P: Provider + Sync + Send,
{
    type Provider = P;

    fn provider(&self) -> &Self::Provider {
        &self.provider
    }

    fn block_id(&self) -> BlockId {
        self.block_id
    }
}

impl<P: Provider + Send + Sync> ClearSigningAccount<'_, P> {
    fn get_invoke_request(
        &self,
        execution: &starknet::accounts::RawExecutionV3,
        query_only: bool,
    ) -> BroadcastedInvokeTransactionV3 {
        use starknet::accounts::ExecutionEncoder;

        BroadcastedInvokeTransactionV3 {
            sender_address: self.address,
            calldata: self.encode_calls(execution.calls()),
            signature: vec![],
            nonce: execution.nonce(),
            resource_bounds: ResourceBoundsMapping {
                l1_gas: ResourceBounds {
                    max_amount: execution.l1_gas(),
                    max_price_per_unit: execution.l1_gas_price(),
                },
                l1_data_gas: ResourceBounds {
                    max_amount: execution.l1_data_gas(),
                    max_price_per_unit: execution.l1_data_gas_price(),
                },
                l2_gas: ResourceBounds {
                    max_amount: execution.l2_gas(),
                    max_price_per_unit: execution.l2_gas_price(),
                },
            },
            tip: 0,
            paymaster_data: vec![],
            account_deployment_data: vec![],
            nonce_data_availability_mode: DataAvailabilityMode::L1,
            fee_data_availability_mode: DataAvailabilityMode::L1,
            is_query: query_only,
        }
    }
}
