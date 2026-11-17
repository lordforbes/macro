//! Credentials of generic (JMAP and IMAP) mailbox links: sealed when stored,
//! opened for each use by the email service.
//!
//! A JMAP API key or IMAP password has no short-lived form, so unlike
//! Microsoft grants there is no token to mint: the email service asks for the
//! secret each time it signs in and drops it straight after.

use generic_mailbox_credentials::{
    cipher::EncryptedMailboxSecret,
    connection::{MailboxServer, MailboxUsername},
};
use generic_mailbox_credentials::{
    cipher::{DataKeyProvider, MailboxSecretCipher, MailboxSecretCipherError},
    credentials::MailboxCredentials,
    store::StoredBinding,
};
use uuid::Uuid;

#[cfg(test)]
mod test;

/// Whether a binding was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingWrite {
    /// The binding is stored.
    Stored,
    /// No generic link with this id and a matching provider exists.
    LinkNotFound,
}

/// Where bindings are kept.
pub trait GenericMailboxBindings: Send + Sync + 'static {
    /// The storage failure.
    type Err: std::error::Error + Send + Sync + 'static;

    /// Store or replace a link's binding.
    fn store(
        &self,
        link_id: Uuid,
        server: &MailboxServer,
        username: &MailboxUsername,
        encrypted: &EncryptedMailboxSecret,
    ) -> impl Future<Output = Result<BindingWrite, Self::Err>> + Send;

    /// A link's binding, if it has one.
    fn binding(
        &self,
        link_id: Uuid,
    ) -> impl Future<Output = Result<Option<StoredBinding>, Self::Err>> + Send;
}

/// Why credentials could not be stored or read.
#[derive(Debug, thiserror::Error)]
pub enum GenericMailboxError {
    /// No generic link with this id, or it has no stored credentials.
    #[error("no generic mailbox credentials for this link")]
    NotFound,
    /// Storage or KMS failed.
    #[error("generic mailbox credentials are temporarily unavailable")]
    Unavailable(rootcause::Report),
}

/// The routes' view of the service. Object-safe (hence `async_trait`) because
/// the API context holds it as `Arc<dyn>`, absent where no KMS key is set.
#[async_trait::async_trait]
pub trait GenericMailboxes: Send + Sync {
    /// Seal and store a link's credentials, replacing earlier ones.
    async fn store(
        &self,
        link_id: Uuid,
        credentials: MailboxCredentials,
    ) -> Result<(), GenericMailboxError>;

    /// Open a link's credentials for one sign-in.
    async fn credentials(&self, link_id: Uuid) -> Result<MailboxCredentials, GenericMailboxError>;
}

/// Seals secrets with a KMS-backed envelope and keeps them in `bindings`.
pub struct GenericMailboxService<R, P> {
    bindings: R,
    cipher: MailboxSecretCipher<P>,
}

impl<R, P: DataKeyProvider> GenericMailboxService<R, P> {
    /// Store bindings in `bindings`, sealed with data keys from `data_keys`.
    pub fn new(bindings: R, data_keys: P) -> Self {
        Self {
            bindings,
            cipher: MailboxSecretCipher::new(data_keys),
        }
    }
}

#[async_trait::async_trait]
impl<R, P> GenericMailboxes for GenericMailboxService<R, P>
where
    R: GenericMailboxBindings,
    P: DataKeyProvider + 'static,
{
    #[tracing::instrument(
        name = "generic_mailbox.store_credentials",
        skip(self, credentials),
        fields(protocol = %credentials.server.protocol()),
        err
    )]
    async fn store(
        &self,
        link_id: Uuid,
        credentials: MailboxCredentials,
    ) -> Result<(), GenericMailboxError> {
        let encrypted = self
            .cipher
            .encrypt(link_id, &credentials.secret)
            .await
            .map_err(cipher_unavailable)?;
        // The plaintext is no longer needed once sealed.
        drop(credentials.secret);
        let written = self
            .bindings
            .store(
                link_id,
                &credentials.server,
                &credentials.username,
                &encrypted,
            )
            .await
            .map_err(unavailable)?;
        match written {
            BindingWrite::Stored => Ok(()),
            BindingWrite::LinkNotFound => Err(GenericMailboxError::NotFound),
        }
    }

    #[tracing::instrument(name = "generic_mailbox.read_credentials", skip(self), err)]
    async fn credentials(&self, link_id: Uuid) -> Result<MailboxCredentials, GenericMailboxError> {
        let binding = self
            .bindings
            .binding(link_id)
            .await
            .map_err(unavailable)?
            .ok_or(GenericMailboxError::NotFound)?;
        let secret = self
            .cipher
            .decrypt(link_id, &binding.encrypted)
            .await
            .map_err(cipher_unavailable)?;
        Ok(MailboxCredentials {
            server: binding.server,
            username: binding.username,
            secret,
        })
    }
}

fn unavailable<E: std::error::Error + Send + Sync + 'static>(error: E) -> GenericMailboxError {
    GenericMailboxError::Unavailable(rootcause::Report::new(error).into_dynamic())
}

fn cipher_unavailable(error: MailboxSecretCipherError) -> GenericMailboxError {
    unavailable(error)
}
