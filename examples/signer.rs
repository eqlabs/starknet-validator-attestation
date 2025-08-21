use axum::{
    Json, Router,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use starknet::core::types::{BroadcastedInvokeTransactionV3, Felt};
use starknet::signers::SigningKey;
use starknet_crypto::{PoseidonHasher, poseidon_hash_many};
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Serialize)]
struct PublicKeyResponse {
    public_key: Felt,
}

#[derive(Deserialize)]
struct SignHashRequest {
    transaction: BroadcastedInvokeTransactionV3,
    chain_id: Felt,
}

#[derive(Serialize)]
struct SignHashResponse {
    signature: [Felt; 2],
}

#[tokio::main]
async fn main() {
    let format = tracing_subscriber::fmt::format().compact();
    tracing_subscriber::fmt()
        .event_format(format)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let signing_key = SigningKey::from_secret_scalar(
        Felt::from_hex(
            &std::env::var("PRIVATE_KEY")
                .expect("PRIVATE_KEY environment variable should be set to the private key"),
        )
        .unwrap(),
    );

    let state = Arc::new(Mutex::new(signing_key));

    let app = Router::new()
        .route(
            "/get_public_key",
            get({
                let state: Arc<Mutex<SigningKey>> = Arc::clone(&state);
                move || async move {
                    Json(PublicKeyResponse {
                        public_key: state.lock().await.verifying_key().scalar(),
                    })
                }
            }),
        )
        .route(
            "/sign",
            post({
                let state = Arc::clone(&state);
                move |Json(payload): Json<SignHashRequest>| {
                    let state = Arc::clone(&state);
                    async move {
                        let transaction_hash = transaction_hash(&payload.transaction, payload.chain_id);
                        tracing::info!(transaction=?payload.transaction, chain_id=?payload.chain_id, ?transaction_hash, "Signing transaction");


                        // Sign the hash
                        let signing_key = state.lock().await;
                        let signature = signing_key.sign(&transaction_hash).unwrap();

                        Json(SignHashResponse {
                            signature: [signature.r, signature.s],
                        })
                    }
                }
            }),
        );

    let listener = tokio::net::TcpListener::bind("localhost:3000")
        .await
        .unwrap();
    axum::serve(listener, app).await.unwrap();
}

/// Cairo string for "invoke"
const PREFIX_INVOKE: Felt = Felt::from_raw([
    513398556346534256,
    18446744073709551615,
    18446744073709551615,
    18443034532770911073,
]);

/// 2 ^ 128 + 3
const QUERY_VERSION_THREE: Felt = Felt::from_raw([
    576460752142432688,
    18446744073709551584,
    17407,
    18446744073700081569,
]);

// Mostly a copy of the `RawExecutionV3::transaction_hash()` method from the starknet-rs.
fn transaction_hash(tx: &BroadcastedInvokeTransactionV3, chain_id: Felt) -> Felt {
    let mut hasher = PoseidonHasher::new();

    hasher.update(PREFIX_INVOKE);
    hasher.update(if tx.is_query {
        QUERY_VERSION_THREE
    } else {
        Felt::THREE
    });
    hasher.update(tx.sender_address);

    hasher.update({
        let mut fee_hasher = PoseidonHasher::new();

        fee_hasher.update(tx.tip.into());

        let mut resource_buffer = [
            0, 0, b'L', b'1', b'_', b'G', b'A', b'S', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        resource_buffer[8..(8 + 8)]
            .copy_from_slice(&tx.resource_bounds.l1_gas.max_amount.to_be_bytes());
        resource_buffer[(8 + 8)..]
            .copy_from_slice(&tx.resource_bounds.l1_gas.max_price_per_unit.to_be_bytes());
        fee_hasher.update(Felt::from_bytes_be(&resource_buffer));

        let mut resource_buffer = [
            0, 0, b'L', b'2', b'_', b'G', b'A', b'S', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        resource_buffer[8..(8 + 8)]
            .copy_from_slice(&tx.resource_bounds.l2_gas.max_amount.to_be_bytes());
        resource_buffer[(8 + 8)..]
            .copy_from_slice(&tx.resource_bounds.l2_gas.max_price_per_unit.to_be_bytes());
        fee_hasher.update(Felt::from_bytes_be(&resource_buffer));

        let mut resource_buffer = [
            0, b'L', b'1', b'_', b'D', b'A', b'T', b'A', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        resource_buffer[8..(8 + 8)]
            .copy_from_slice(&tx.resource_bounds.l1_data_gas.max_amount.to_be_bytes());
        resource_buffer[(8 + 8)..].copy_from_slice(
            &tx.resource_bounds
                .l1_data_gas
                .max_price_per_unit
                .to_be_bytes(),
        );
        fee_hasher.update(Felt::from_bytes_be(&resource_buffer));

        fee_hasher.finalize()
    });

    hasher.update(poseidon_hash_many(&tx.paymaster_data));

    hasher.update(chain_id);
    hasher.update(tx.nonce);

    // Hard-coded L1 DA mode for nonce and fee
    hasher.update(Felt::ZERO);

    hasher.update(poseidon_hash_many(&tx.account_deployment_data));

    hasher.update(poseidon_hash_many(&tx.calldata));

    hasher.finalize()
}

#[cfg(test)]
mod tests {
    use starknet::core::{
        chain_id,
        types::{
            BroadcastedInvokeTransactionV3, DataAvailabilityMode, ResourceBounds,
            ResourceBoundsMapping,
        },
    };
    use starknet::macros::felt;

    use super::transaction_hash;

    #[test]
    fn test_transaction_hash() {
        let tx = BroadcastedInvokeTransactionV3 {
            sender_address: felt!(
                "0x2e216b191ac966ba1d35cb6cfddfaf9c12aec4dfe869d9fa6233611bb334ee9"
            ),
            calldata: vec![
                felt!("0x1"),
                felt!("0x3f32e152b9637c31bfcf73e434f78591067a01ba070505ff6ee195642c9acfb"),
                felt!("0x37446750a403c1b4014436073cf8d08ceadc5b156ac1c8b7b0ca41a0c9c1c54"),
                felt!("0x1"),
                felt!("0x7979a0a0a175d7e738e8e9ba6fa6d48f680d67758f719390eee58e790819836"),
            ],
            signature: vec![],
            nonce: felt!("0x106"),
            resource_bounds: ResourceBoundsMapping {
                l1_gas: ResourceBounds {
                    max_amount: 0,
                    max_price_per_unit: 0x51066a69ad72c,
                },
                l1_data_gas: ResourceBounds {
                    max_amount: 0x600,
                    max_price_per_unit: 0x1254,
                },
                l2_gas: ResourceBounds {
                    max_amount: 0xf00000,
                    max_price_per_unit: 0x308c5bff6,
                },
            },
            tip: 0,
            paymaster_data: vec![],
            account_deployment_data: vec![],
            nonce_data_availability_mode: DataAvailabilityMode::L1,
            fee_data_availability_mode: DataAvailabilityMode::L1,
            is_query: false,
        };
        let chain_id = chain_id::SEPOLIA;
        let tx_hash = transaction_hash(&tx, chain_id);
        assert_eq!(
            tx_hash,
            felt!("0x382a7406fe3931ba1faf00d1eaa36b7c8770b8d185b091b730ecdb4dba5f3ce"),
        );
    }
}
