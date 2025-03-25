use anyhow::Context;
use starknet::{
    core::{
        types::{BlockId, BlockTag, Felt, FunctionCall, NonZeroFelt},
        utils::get_selector_from_name,
    },
    providers::{JsonRpcClient, Provider, jsonrpc::JsonRpcTransport},
};
use starknet_crypto::PoseidonHasher;

#[derive(Debug)]
pub struct AttestationInfo {
    pub staker_address: Felt,
    pub stake: u128,
    pub epoch_len: u64,
    pub epoch_id: u64,
    pub current_epoch_starting_block: u64,
}

impl AttestationInfo {
    pub fn next_epoch(&self) -> Self {
        Self {
            staker_address: self.staker_address,
            stake: self.stake,
            epoch_len: self.epoch_len,
            epoch_id: self.epoch_id + 1,
            current_epoch_starting_block: self.current_epoch_starting_block + self.epoch_len,
        }
    }
}

pub async fn get_attestation_info<T: JsonRpcTransport + Send + Sync + 'static>(
    provider: &JsonRpcClient<T>,
    operational_address: Felt,
) -> anyhow::Result<AttestationInfo> {
    let attestation_info = provider
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
    })
}

pub async fn get_attestation_window<T: JsonRpcTransport + Send + Sync + 'static>(
    provider: &JsonRpcClient<T>,
) -> anyhow::Result<u16> {
    let result = provider
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

pub fn calculate_expected_attestation_block(
    attestation_info: &AttestationInfo,
    attestation_window: u16,
) -> anyhow::Result<u64> {
    let mut h = PoseidonHasher::new();
    h.update(attestation_info.stake.into());
    h.update(attestation_info.epoch_id.into());
    h.update(attestation_info.staker_address);
    let hash = h.finalize();

    let modulus = Felt::from(attestation_info.epoch_len - attestation_window as u64);

    let block_offset: u64 = hash
        .div_rem(&NonZeroFelt::try_from(modulus)?)
        .1
        .try_into()?;

    Ok(attestation_info.current_epoch_starting_block + block_offset)
}
