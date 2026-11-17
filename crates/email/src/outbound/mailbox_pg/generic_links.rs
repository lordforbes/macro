//! Link rows for generic (JMAP and IMAP) mailboxes.

#[cfg(test)]
mod test;

use models_email::service::link::UserProvider;
use uuid::Uuid;

use super::{PgMailboxSync, db_error};
use crate::domain::mailbox::{
    MailboxError,
    generic_connection::{ConnectGenericMailbox, GenericLinkRepository, LinkCreation},
};

impl GenericLinkRepository for PgMailboxSync {
    type Err = MailboxError;

    async fn create_inactive_link(
        &self,
        id: Uuid,
        request: &ConnectGenericMailbox,
        provider: UserProvider,
    ) -> Result<LinkCreation, MailboxError> {
        // Addresses are stored lower-case, as every other link writer does.
        let email_address = request.email_address.0.as_ref().to_lowercase();
        let inserted = sqlx::query_scalar!(
            r#"
            INSERT INTO email_links (id, macro_id, fusionauth_user_id, email_address, provider, is_sync_active)
            VALUES ($1, $2, $3, $4, $5::text::email_user_provider_enum, false)
            ON CONFLICT (fusionauth_user_id, email_address, provider) DO NOTHING
            RETURNING id
            "#,
            id,
            request.owner.as_ref(),
            request.credential_owner,
            email_address,
            provider.as_str(),
        )
        .fetch_optional(&self.db)
        .await
        .map_err(db_error)?;
        Ok(match inserted {
            Some(_) => LinkCreation::Created,
            None => LinkCreation::AlreadyConnected,
        })
    }

    async fn activate(&self, id: Uuid) -> Result<bool, MailboxError> {
        let mut tx = self.db.begin().await.map_err(db_error)?;
        let generation = sqlx::query_scalar!(
            r#"
            UPDATE email_links SET is_sync_active = true, updated_at = now()
            WHERE id = $1 AND provider IN ('JMAP', 'IMAP') AND NOT is_sync_active
                AND disconnect_requested_at IS NULL
            RETURNING sync_generation
            "#,
            id,
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_error)?;
        let Some(generation) = generation else {
            return Ok(false);
        };
        // The engine's folder discovery creates a stream per folder from here.
        sqlx::query!(
            r#"
            INSERT INTO email_sync_streams (id, link_id, generation, kind, scope_id)
            VALUES ($1, $2, $3, 'folder_catalog', 'mail') ON CONFLICT DO NOTHING
            "#,
            macro_uuid::generate_uuid_v7(),
            id,
            generation,
        )
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;
        tx.commit().await.map_err(db_error)?;
        Ok(true)
    }

    async fn delete_link(&self, id: Uuid) -> Result<(), MailboxError> {
        // Only an inactive generic link: never a link that has started syncing.
        sqlx::query!(
            "DELETE FROM email_links WHERE id = $1 AND provider IN ('JMAP', 'IMAP') AND NOT is_sync_active",
            id,
        )
        .execute(&self.db)
        .await
        .map_err(db_error)?;
        Ok(())
    }
}
