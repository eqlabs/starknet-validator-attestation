use axum::{
    Json, Router,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use starknet::signers::SigningKey;
use starknet_core::types::BroadcastedInvokeTransactionV3;
use starknet_crypto::Felt;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Serialize)]
struct PublicKeyResponse {
    public_key: Felt,
}

#[derive(Deserialize)]
struct SignHashRequest {
    transaction_hash: Felt,
    transaction: BroadcastedInvokeTransactionV3,
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
                        tracing::info!(transaction=?payload.transaction, "Signing transaction");
                        // Sign the hash
                        let signing_key = state.lock().await;
                        let signature = signing_key.sign(&payload.transaction_hash).unwrap();

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
