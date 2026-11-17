//! Encrypting a mailbox secret: an AES-256-GCM envelope under a KMS data key.
//!
//! The same scheme as Microsoft refresh tokens, and the same KMS key, which
//! the authentication service already holds `kms:GenerateDataKey` and
//! `kms:Decrypt` on. Two things keep the two uses apart:
//!
//! - **The encryption context names the purpose and the link.** KMS refuses
//!   to decrypt a data key under any other context, with a CloudTrail record
//!   either way, so a refresh token cannot be read as a mailbox secret and a
//!   row copied to another link does not decrypt.
//! - **The same values are the AES-GCM associated data,** so a ciphertext
//!   moved between rows that share a data key still fails to open.

#[cfg(test)]
mod test;

use std::collections::HashMap;

use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, AeadCore, KeyInit, OsRng, Payload},
};
use aws_sdk_kms::{Client, primitives::Blob, types::DataKeySpec};
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::secret::MailboxSecret;

/// The encryption scheme this crate writes, stored on every row.
pub const ENCRYPTION_VERSION: i16 = 1;

/// Names the purpose in the encryption context and associated data.
const ENCRYPTION_PURPOSE: &str = "generic-mailbox-secret";

const AES_256_KEY_LENGTH: usize = 32;
const AES_GCM_NONCE_LENGTH: usize = 12;
const AES_GCM_TAG_LENGTH: usize = 16;

/// An encrypted secret, ready to store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncryptedMailboxSecret {
    /// The AES-GCM ciphertext and tag.
    pub secret_ciphertext: Vec<u8>,
    /// The data key, encrypted by KMS.
    pub encrypted_data_key: Vec<u8>,
    /// The AES-GCM nonce.
    pub nonce: Vec<u8>,
    /// The scheme that produced the envelope.
    pub encryption_version: i16,
    /// The KMS key that encrypted the data key.
    pub kms_key_id: String,
}

/// A fresh data key from KMS: its plaintext, and the same key encrypted.
pub struct GeneratedDataKey {
    /// The plaintext data key. Zeroized on drop.
    pub plaintext: Zeroizing<Vec<u8>>,
    /// The data key encrypted under the KMS key.
    pub encrypted: Vec<u8>,
    /// The KMS key that encrypted it.
    pub key_id: String,
}

/// The KMS operations the envelope needs, as a seam tests can stand in for.
pub trait DataKeyProvider: Send + Sync {
    /// Generate a 256-bit data key under `encryption_context`.
    fn generate_data_key(
        &self,
        encryption_context: HashMap<String, String>,
    ) -> impl Future<Output = Result<GeneratedDataKey, DataKeyError>> + Send;

    /// Decrypt a data key, which KMS refuses unless `encryption_context`
    /// matches the one it was generated under.
    fn decrypt_data_key(
        &self,
        key_id: &str,
        encrypted_data_key: &[u8],
        encryption_context: HashMap<String, String>,
    ) -> impl Future<Output = Result<Zeroizing<Vec<u8>>, DataKeyError>> + Send;
}

/// A KMS call failed. Opaque on purpose: telling "wrong context" from "no
/// permission" helps a prober more than a caller.
#[derive(Debug, thiserror::Error)]
#[error("generic mailbox secret kms operation failed")]
pub struct DataKeyError;

/// Why a secret could not be encrypted or decrypted.
#[derive(Debug, thiserror::Error)]
pub enum MailboxSecretCipherError {
    /// The KMS call failed, including because the context did not match,
    /// which is what a row moved between links looks like.
    #[error("generic mailbox secret kms operation failed")]
    Kms(#[from] DataKeyError),
    /// KMS returned a data key of the wrong length.
    #[error("generic mailbox secret data key is invalid")]
    InvalidDataKey,
    /// The stored envelope is incomplete.
    #[error("generic mailbox secret envelope is malformed")]
    MalformedEnvelope,
    /// The envelope did not open: tampered, or bound to another link.
    #[error("generic mailbox secret could not be decrypted")]
    DecryptionFailed,
    /// Encryption failed.
    #[error("generic mailbox secret could not be encrypted")]
    EncryptionFailed,
    /// The decrypted bytes are not a secret this crate would have stored.
    #[error("generic mailbox secret plaintext is malformed")]
    MalformedPlaintext,
    /// The row was written by a scheme this build does not know.
    #[error("generic mailbox secret uses unsupported encryption version {0}")]
    UnsupportedVersion(i16),
}

/// Encrypts and decrypts mailbox secrets, each bound to one link.
pub struct MailboxSecretCipher<P> {
    data_keys: P,
}

impl<P: DataKeyProvider> MailboxSecretCipher<P> {
    /// Encrypt with data keys from `data_keys`.
    pub fn new(data_keys: P) -> Self {
        Self { data_keys }
    }

    /// Encrypt `secret` so that it decrypts only for `link_id`.
    pub async fn encrypt(
        &self,
        link_id: Uuid,
        secret: &MailboxSecret,
    ) -> Result<EncryptedMailboxSecret, MailboxSecretCipherError> {
        let binding = Binding(link_id);
        let data_key = self
            .data_keys
            .generate_data_key(binding.encryption_context())
            .await?;
        let cipher = aes(&data_key.plaintext)?;
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let secret_ciphertext = cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: secret.expose().as_bytes(),
                    aad: &binding.aad(),
                },
            )
            .map_err(|_| MailboxSecretCipherError::EncryptionFailed)?;
        Ok(EncryptedMailboxSecret {
            secret_ciphertext,
            encrypted_data_key: data_key.encrypted,
            nonce: nonce.to_vec(),
            encryption_version: ENCRYPTION_VERSION,
            kms_key_id: data_key.key_id,
        })
    }

    /// Decrypt a secret stored for `link_id`.
    pub async fn decrypt(
        &self,
        link_id: Uuid,
        envelope: &EncryptedMailboxSecret,
    ) -> Result<MailboxSecret, MailboxSecretCipherError> {
        // Checked before KMS: a malformed row is our bug to report, not a
        // request worth a KMS call.
        validate(envelope)?;
        let binding = Binding(link_id);
        let data_key = self
            .data_keys
            .decrypt_data_key(
                &envelope.kms_key_id,
                &envelope.encrypted_data_key,
                binding.encryption_context(),
            )
            .await?;
        let cipher = aes(&data_key)?;
        let nonce_bytes: [u8; AES_GCM_NONCE_LENGTH] = envelope
            .nonce
            .as_slice()
            .try_into()
            .map_err(|_| MailboxSecretCipherError::MalformedEnvelope)?;
        let plaintext = cipher
            .decrypt(
                &Nonce::from(nonce_bytes),
                Payload {
                    msg: &envelope.secret_ciphertext,
                    aad: &binding.aad(),
                },
            )
            .map_err(|_| MailboxSecretCipherError::DecryptionFailed)?;
        let plaintext = String::from_utf8(plaintext).map_err(|error| {
            // The rejected bytes are the secret; wipe them.
            drop(Zeroizing::new(error.into_bytes()));
            MailboxSecretCipherError::MalformedPlaintext
        })?;
        MailboxSecret::parse(plaintext).map_err(|_| MailboxSecretCipherError::MalformedPlaintext)
    }
}

fn aes(data_key: &[u8]) -> Result<Aes256Gcm, MailboxSecretCipherError> {
    if data_key.len() != AES_256_KEY_LENGTH {
        return Err(MailboxSecretCipherError::InvalidDataKey);
    }
    Aes256Gcm::new_from_slice(data_key).map_err(|_| MailboxSecretCipherError::InvalidDataKey)
}

fn validate(envelope: &EncryptedMailboxSecret) -> Result<(), MailboxSecretCipherError> {
    if envelope.encryption_version != ENCRYPTION_VERSION {
        return Err(MailboxSecretCipherError::UnsupportedVersion(
            envelope.encryption_version,
        ));
    }
    let complete = envelope.nonce.len() == AES_GCM_NONCE_LENGTH
        && envelope.secret_ciphertext.len() > AES_GCM_TAG_LENGTH
        && !envelope.encrypted_data_key.is_empty()
        && !envelope.kms_key_id.trim().is_empty();
    if !complete {
        return Err(MailboxSecretCipherError::MalformedEnvelope);
    }
    Ok(())
}

/// The link a ciphertext is bound to. One constructor, so the context cannot
/// be built two ways in two places.
struct Binding(Uuid);

impl Binding {
    fn encryption_context(&self) -> HashMap<String, String> {
        HashMap::from([
            ("macro:purpose".to_owned(), ENCRYPTION_PURPOSE.to_owned()),
            (
                "macro:encryption-version".to_owned(),
                ENCRYPTION_VERSION.to_string(),
            ),
            ("macro:email-link-id".to_owned(), self.0.to_string()),
        ])
    }

    fn aad(&self) -> Vec<u8> {
        let mut aad = ENCRYPTION_PURPOSE.as_bytes().to_vec();
        aad.extend_from_slice(&ENCRYPTION_VERSION.to_be_bytes());
        aad.extend_from_slice(self.0.as_bytes());
        aad
    }
}

/// [`DataKeyProvider`] over a real AWS KMS key.
#[derive(Clone)]
pub struct AwsKmsDataKeys {
    client: Client,
    key_id: String,
}

impl AwsKmsDataKeys {
    /// Generate data keys under `key_id`, and decrypt them.
    pub fn new(client: Client, key_id: String) -> Self {
        Self { client, key_id }
    }
}

impl DataKeyProvider for AwsKmsDataKeys {
    async fn generate_data_key(
        &self,
        encryption_context: HashMap<String, String>,
    ) -> Result<GeneratedDataKey, DataKeyError> {
        let output = self
            .client
            .generate_data_key()
            .key_id(&self.key_id)
            .key_spec(DataKeySpec::Aes256)
            .set_encryption_context(Some(encryption_context))
            .send()
            .await
            .map_err(|_| DataKeyError)?;
        let plaintext = output
            .plaintext()
            .map(|value| Zeroizing::new(value.as_ref().to_vec()))
            .ok_or(DataKeyError)?;
        let encrypted = output
            .ciphertext_blob()
            .map(|value| value.as_ref().to_vec())
            .ok_or(DataKeyError)?;
        // The response names the key by ARN even when configured by alias,
        // which is what a row should record.
        let key_id = output
            .key_id()
            .filter(|value| !value.trim().is_empty())
            .map(str::to_owned)
            .ok_or(DataKeyError)?;
        Ok(GeneratedDataKey {
            plaintext,
            encrypted,
            key_id,
        })
    }

    async fn decrypt_data_key(
        &self,
        key_id: &str,
        encrypted_data_key: &[u8],
        encryption_context: HashMap<String, String>,
    ) -> Result<Zeroizing<Vec<u8>>, DataKeyError> {
        let output = self
            .client
            .decrypt()
            // Named explicitly, so a blob under some other key we can reach
            // is not decrypted here.
            .key_id(key_id)
            .ciphertext_blob(Blob::new(encrypted_data_key))
            .set_encryption_context(Some(encryption_context))
            .send()
            .await
            .map_err(|_| DataKeyError)?;
        output
            .plaintext()
            .map(|value| Zeroizing::new(value.as_ref().to_vec()))
            .ok_or(DataKeyError)
    }
}
