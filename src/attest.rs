use anyhow::Context;
use starknet::{
    accounts::{Account, ExecutionEncoding, SingleOwnerAccount},
    core::{
        chain_id,
        types::{BlockId, BlockTag, Call, FunctionCall},
        utils::get_selector_from_name,
    },
    providers::{JsonRpcClient, Provider, jsonrpc::JsonRpcTransport},
    signers::LocalWallet,
};
use starknet_crypto::Felt;

pub async fn attest<T: JsonRpcTransport + Send + Sync + 'static>(
    provider: &JsonRpcClient<T>,
    signer: &LocalWallet,
    block_hash: Felt,
) -> anyhow::Result<Felt> {
    let mut account = SingleOwnerAccount::new(
        provider,
        signer,
        crate::config::STAKER_OPERATIONAL_ADDRESS,
        chain_id::SEPOLIA,
        ExecutionEncoding::New,
    );

    account.set_block_id(BlockId::Tag(BlockTag::Pending));

    let result = account
        .execute_v3(vec![Call {
            to: crate::config::ATTESTATION_CONTRACT_ADDRESS,
            selector: get_selector_from_name("attest").unwrap(),
            calldata: vec![block_hash],
        }])
        .send()
        .await
        .context("Sending attestation transaction")?;

    Ok(result.transaction_hash)
}

pub async fn attestation_done_in_current_epoch<T: JsonRpcTransport + Send + Sync + 'static>(
    provider: &JsonRpcClient<T>,
    staker_address: Felt,
) -> anyhow::Result<bool> {
    let result = provider
        .call(
            FunctionCall {
                contract_address: crate::config::ATTESTATION_CONTRACT_ADDRESS,
                entry_point_selector: get_selector_from_name("is_attestation_done_in_curr_epoch")
                    .unwrap(),
                calldata: vec![staker_address],
            },
            BlockId::Tag(BlockTag::Pending),
        )
        .await
        .unwrap();

    Ok(result == vec![Felt::ONE])
}
