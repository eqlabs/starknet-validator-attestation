use starknet::{core::types::Felt, macros::felt};

pub const STAKING_CONTRACT_ADDRESS: Felt =
    felt!("0x034370fc9931c636ab07b16ada82d60f05d32993943debe2376847e0921c1162");
pub const ATTESTATION_CONTRACT_ADDRESS: Felt =
    felt!("0x04862e05d00f2d0981c4a912269c21ad99438598ab86b6e70d1cee267caaa78d");
pub const STAKER_OPERATIONAL_ADDRESS: Felt =
    felt!("0x02E216b191Ac966Ba1d35Cb6cfdDFaF9C12AEc4DFE869d9FA6233611bb334EE9");
pub const NODE_URL_WS: &str = "ws://127.0.0.1:9545/rpc/v0_8";
pub const NODE_URL_HTTP: &str = "http://127.0.0.1:9545/rpc/v0_8";
pub const MIN_ATTESTATION_WINDOW: u64 = 10;
