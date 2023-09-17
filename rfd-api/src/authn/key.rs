use argon2::password_hash::rand_core::{OsRng, RngCore};
use hex::FromHexError;
use thiserror::Error;
use uuid::Uuid;

use super::{Signer, SigningKeyError};

pub struct RawApiKey {
    id: Uuid,
    clear: String,
}

#[derive(Debug, Error)]
pub enum ApiKeyError {
    #[error("Failed to decode signature: {0}")]
    Decode(#[from] FromHexError),
    #[error("Failed to parse API key")]
    FailedToParse,
    #[error("Signature is malformed: {0}")]
    MalformedSignature(#[from] rsa::signature::Error),
    #[error("Failed to sign API key: {0}")]
    Signing(SigningKeyError),
}

impl RawApiKey {
    // Generate a new API key
    pub fn generate<const N: usize>(id: &Uuid) -> Self {
        // Generate random data to extend the token id with
        let mut token_raw = [0; N];
        OsRng.fill_bytes(&mut token_raw);

        let clear = hex::encode(token_raw);

        Self { id: *id, clear }
    }

    pub fn id(&self) -> &Uuid {
        &self.id
    }

    pub async fn sign(self, signer: &dyn Signer) -> Result<SignedApiKey, ApiKeyError> {
        let key = format!("{}.{}", self.id, self.clear);
        let signature = hex::encode(
            signer
                .sign(&key)
                .await
                .map_err(ApiKeyError::Signing)?,
        );
        Ok(SignedApiKey::new(key, signature))
    }
}

impl TryFrom<&str> for RawApiKey {
    type Error = ApiKeyError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value.split_once(".") {
            Some((id, key)) => {
                Ok(RawApiKey {
                    id: id.parse().map_err(|err| {
                        tracing::info!(?err, "Api key prefix is not a valid uuid");
                        ApiKeyError::FailedToParse
                    })?,
                    clear: key.to_string(),
                })
            }
            None => {
                Err(ApiKeyError::FailedToParse)
            }
        }
    }
}

pub struct SignedApiKey {
    key: String,
    signature: String,
}

impl SignedApiKey {
    fn new(key: String, signature: String) -> Self {
        Self {
            key,
            signature,
        }
    }

    pub fn key(self) -> String {
        self.key
    }

    pub fn signature(&self) -> &str {
        &self.signature
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::RawApiKey;
    use crate::util::tests::mock_key;

    #[tokio::test]
    async fn test_rejects_invalid_source() {
        let id = Uuid::new_v4();
        let signer = mock_key().as_signer().await.unwrap();

        let raw1 = RawApiKey::generate::<8>(&id);
        let signed1 = raw1.sign(&*signer).await.unwrap();

        let raw2 = RawApiKey::generate::<8>(&id);
        let signed2 = raw2.sign(&*signer).await.unwrap();

        assert_ne!(signed1.signature(), signed2.signature())
    }
}
