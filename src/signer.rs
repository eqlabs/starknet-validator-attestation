use serde::{Deserialize, Serialize};
use starknet::signers::{LocalWallet, Signer, SignerInteractivityContext, VerifyingKey};
use starknet_crypto::{Felt, Signature};

pub enum AttestationSigner {
    Local(LocalWallet),
    Remote(RemoteSigner),
}

impl AttestationSigner {
    pub fn new_local(wallet: LocalWallet) -> Self {
        Self::Local(wallet)
    }

    pub fn new_remote(url: url::Url) -> Self {
        Self::Remote(RemoteSigner::new(url))
    }
}

#[derive(Debug, Clone)]
pub struct RemoteSigner {
    url: url::Url,
    client: reqwest::Client,
}

impl RemoteSigner {
    /// Constructs [`RemoteSigner`] from a [`reqwest::Client`].
    pub fn new(url: url::Url) -> Self {
        Self {
            url,
            client: reqwest::Client::new(),
        }
    }
}

/// Errors using [`LocalWallet`].
#[derive(Debug, thiserror::Error)]
pub enum RemoteSignerError {
    /// ECDSA signature error.
    #[error(transparent)]
    TransportError(reqwest::Error),
}

#[async_trait::async_trait]
impl Signer for RemoteSigner {
    type GetPublicKeyError = RemoteSignerError;
    type SignError = RemoteSignerError;

    async fn get_public_key(&self) -> Result<VerifyingKey, Self::GetPublicKeyError> {
        let public_key = self
            .client
            .get(self.url.join("/get_public_key").unwrap())
            .send()
            .await
            .map_err(RemoteSignerError::TransportError)?
            .json::<PublicKeyResponse>()
            .await
            .map_err(RemoteSignerError::TransportError)?
            .public_key;
        Ok(VerifyingKey::from_scalar(public_key))
    }

    async fn sign_hash(&self, hash: &Felt) -> Result<Signature, Self::SignError> {
        let signature = self
            .client
            .post(self.url.join("/sign_hash").unwrap())
            .json(&SignHashRequest { hash: *hash })
            .send()
            .await
            .map_err(RemoteSignerError::TransportError)?
            .json::<SignHashResponse>()
            .await
            .map_err(RemoteSignerError::TransportError)?
            .signature;
        Ok(Signature {
            r: signature[0],
            s: signature[1],
        })
    }

    fn is_interactive(&self, _context: SignerInteractivityContext<'_>) -> bool {
        true
    }
}

#[derive(Deserialize)]
struct PublicKeyResponse {
    public_key: Felt,
}

#[derive(Serialize)]
struct SignHashRequest {
    hash: Felt,
}

#[derive(Deserialize)]
struct SignHashResponse {
    signature: [Felt; 2],
}
