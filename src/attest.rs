use anyhow::Context;
use starknet::{
    accounts::{Account, ExecutionEncoding, SingleOwnerAccount},
    core::{
        chain_id,
        types::{BlockId, BlockTag, Call},
        utils::get_selector_from_name,
    },
    providers::{JsonRpcClient, jsonrpc::JsonRpcTransport},
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
