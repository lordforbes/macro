//! Postgres storage for generic mailbox bindings.

use generic_mailbox_credentials::{
    cipher::EncryptedMailboxSecret,
    connection::{MailboxServer, MailboxUsername},
    store::{BindingStoreError, StoredBinding, get_binding, upsert_binding},
};
use sqlx::PgPool;
use uuid::Uuid;

use crate::domain::generic_mailbox::{BindingWrite, GenericMailboxBindings};

/// Bindings in `email_generic_mailbox_bindings`.
pub struct PgGenericMailboxBindings(PgPool);

impl PgGenericMailboxBindings {
    pub(crate) fn new(db: PgPool) -> Self {
        Self(db)
    }
}

impl GenericMailboxBindings for PgGenericMailboxBindings {
    type Err = BindingStoreError;

    async fn store(
        &self,
        link_id: Uuid,
        server: &MailboxServer,
        username: &MailboxUsername,
        encrypted: &EncryptedMailboxSecret,
    ) -> Result<BindingWrite, BindingStoreError> {
        match upsert_binding(&self.0, link_id, server, username, encrypted).await {
            Ok(()) => Ok(BindingWrite::Stored),
            Err(BindingStoreError::LinkNotFound) => Ok(BindingWrite::LinkNotFound),
            Err(error) => Err(error),
        }
    }

    async fn binding(&self, link_id: Uuid) -> Result<Option<StoredBinding>, BindingStoreError> {
        get_binding(&self.0, link_id).await
    }
}
