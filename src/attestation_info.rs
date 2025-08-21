use starknet::core::types::{Felt, NonZeroFelt};
use starknet_crypto::PoseidonHasher;

#[derive(Clone, Debug, PartialEq)]
pub struct AttestationInfo {
    pub staker_address: Felt,
    pub operational_address: Felt,
    pub stake: u128,
    pub epoch_len: u64,
    pub epoch_id: u64,
    pub current_epoch_starting_block: u64,
    pub attestation_window: u16,
}

impl AttestationInfo {
    pub fn calculate_expected_attestation_block(&self) -> u64 {
        let mut h = PoseidonHasher::new();
        h.update(self.stake.into());
        h.update(self.epoch_id.into());
        h.update(self.staker_address);
        let hash = h.finalize();

        let modulus = Felt::from(self.epoch_len - self.attestation_window as u64);

        let block_offset: u64 = hash
            .div_rem(&NonZeroFelt::try_from(modulus).expect("Modulus is not zero"))
            .1
            .try_into()
            .expect("Modulus is less than u64::MAX");

        self.current_epoch_starting_block + block_offset
    }
}
