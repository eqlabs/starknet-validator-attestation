use serde::{Deserialize, Serialize};
use starknet::signers::{LocalWallet, Signer, SignerInteractivityContext};
use starknet_core::types::BroadcastedInvokeTransactionV3;
use starknet_crypto::{Felt, Signature};

#[derive(Debug, thiserror::Error)]
pub enum SignError {
    /// An error encountered by the signer implementation.
    #[error(transparent)]
    Signing(starknet_core::crypto::EcdsaSignError),
    /// A transport error encountered during remote signing.
    #[error(transparent)]
    Transport(reqwest::Error),
}

impl From<starknet::signers::local_wallet::SignError> for SignError {
    fn from(value: starknet::signers::local_wallet::SignError) -> Self {
        match value {
            starknet::signers::local_wallet::SignError::EcdsaSignError(ecdsa_sign_error) => {
                Self::Signing(ecdsa_sign_error)
            }
        }
    }
}

#[derive(Debug, Clone)]
pub enum AttestationSigner {
    Local(LocalWallet),
    Remote(RemoteSigner),
}

impl AttestationSigner {
    pub fn new_local(wallet: LocalWallet) -> Self {
        Self::Local(wallet)
    }

    pub fn new_remote(url: url::Url) -> anyhow::Result<Self> {
        Ok(Self::Remote(RemoteSigner::new(url)?))
    }

    pub async fn sign(
        &self,
        hash: &Felt,
        transaction: BroadcastedInvokeTransactionV3,
        chain_id: Felt,
    ) -> Result<Signature, SignError> {
        let signature = match self {
            Self::Local(wallet) => wallet.sign_hash(hash).await?,
            Self::Remote(signer) => signer.sign(transaction, chain_id).await?,
        };
        Ok(signature)
    }

    pub fn is_signer_interactive(&self, context: SignerInteractivityContext<'_>) -> bool {
        match self {
            Self::Local(_) => false,
            Self::Remote(signer) => signer.is_interactive(context),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RemoteSigner {
    url: url::Url,
    client: reqwest::Client,
}

impl RemoteSigner {
    /// Constructs [`RemoteSigner`] from a [`reqwest::Client`].
    pub fn new(url: url::Url) -> anyhow::Result<Self> {
        Ok(Self {
            url,
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .build()?,
        })
    }
}

impl RemoteSigner {
    async fn sign(
        &self,
        transaction: BroadcastedInvokeTransactionV3,
        chain_id: Felt,
    ) -> Result<Signature, SignError> {
        let signature = self
            .client
            .post(self.url.join("/sign").unwrap())
            .json(&SignRequest {
                transaction,
                chain_id,
            })
            .send()
            .await
            .map_err(SignError::Transport)?
            .json::<SignHashResponse>()
            .await
            .map_err(SignError::Transport)?
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

#[derive(Serialize)]
struct SignRequest {
    transaction: BroadcastedInvokeTransactionV3,
    chain_id: Felt,
}

#[derive(Deserialize)]
struct SignHashResponse {
    signature: [Felt; 2],
}
