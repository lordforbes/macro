//! Reading and writing the `email_generic_mailbox_bindings` row.
//!
//! Only ciphertext passes through here. Turning it into a usable secret needs
//! [`crate::cipher`], so every read of a plaintext secret is a KMS call.

#[cfg(test)]
mod test;

use std::num::NonZeroU16;

use uuid::Uuid;

use crate::{
    cipher::EncryptedMailboxSecret,
    connection::{
        ImapHost, ImapTls, InvalidConnection, JmapOrigin, MailboxProtocol, MailboxServer,
        MailboxUsername,
    },
};

/// A generic mailbox's connection settings and encrypted secret.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredBinding {
    /// The server the mailbox is read from.
    pub server: MailboxServer,
    /// The name it signs in with.
    pub username: MailboxUsername,
    /// The encrypted secret.
    pub encrypted: EncryptedMailboxSecret,
}

/// Why a binding could not be read or written.
#[derive(Debug, thiserror::Error)]
pub enum BindingStoreError {
    /// No link with this id and a matching generic provider exists.
    #[error("no generic mailbox link with this id")]
    LinkNotFound,
    /// A stored row no longer parses; a bug, not a user error.
    #[error("stored generic mailbox binding is invalid")]
    Corrupt(#[from] InvalidConnection),
    /// The database call failed.
    #[error("generic mailbox binding storage failed")]
    Database(#[from] sqlx::Error),
}

/// Store a link's connection settings and secret, replacing any earlier ones.
///
/// Replacing is what a user changing their password wants. The write happens
/// only when the link exists and its provider is the server's protocol, so a
/// secret cannot be attached to a Gmail or Outlook link.
///
/// # Errors
/// [`BindingStoreError::LinkNotFound`] when there is no such generic link.
pub async fn upsert_binding(
    executor: impl sqlx::PgExecutor<'_>,
    link_id: Uuid,
    server: &MailboxServer,
    username: &MailboxUsername,
    encrypted: &EncryptedMailboxSecret,
) -> Result<(), BindingStoreError> {
    let protocol = server.protocol();
    let (endpoint, host, port, tls_mode) = match server {
        MailboxServer::Jmap { endpoint } => (Some(endpoint.as_str()), None, None, None),
        MailboxServer::Imap { host, port, tls } => (
            None,
            Some(host.as_str()),
            Some(i32::from(port.get())),
            Some(tls.as_ref()),
        ),
    };
    let written = sqlx::query_scalar!(
        r#"
        INSERT INTO email_generic_mailbox_bindings (
            link_id, protocol, endpoint, host, port, tls_mode, username,
            secret_ciphertext, encrypted_data_key, nonce, encryption_version, kms_key_id
        )
        SELECT l.id, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12
        FROM email_links l
        WHERE l.id = $1 AND l.provider::text = upper($2)
        ON CONFLICT (link_id) DO UPDATE SET
            protocol = EXCLUDED.protocol,
            endpoint = EXCLUDED.endpoint,
            host = EXCLUDED.host,
            port = EXCLUDED.port,
            tls_mode = EXCLUDED.tls_mode,
            username = EXCLUDED.username,
            secret_ciphertext = EXCLUDED.secret_ciphertext,
            encrypted_data_key = EXCLUDED.encrypted_data_key,
            nonce = EXCLUDED.nonce,
            encryption_version = EXCLUDED.encryption_version,
            kms_key_id = EXCLUDED.kms_key_id,
            updated_at = now()
        RETURNING link_id
        "#,
        link_id,
        protocol.as_ref(),
        endpoint,
        host,
        port,
        tls_mode,
        username.as_str(),
        &encrypted.secret_ciphertext,
        &encrypted.encrypted_data_key,
        &encrypted.nonce,
        encrypted.encryption_version,
        encrypted.kms_key_id,
    )
    .fetch_optional(executor)
    .await?;
    written.map(|_| ()).ok_or(BindingStoreError::LinkNotFound)
}

/// A link's binding, or `None` when it has none.
///
/// # Errors
/// When the row cannot be read, or no longer parses.
pub async fn get_binding(
    executor: impl sqlx::PgExecutor<'_>,
    link_id: Uuid,
) -> Result<Option<StoredBinding>, BindingStoreError> {
    let Some(row) = sqlx::query!(
        r#"
        SELECT protocol, endpoint, host, port, tls_mode, username,
            secret_ciphertext, encrypted_data_key, nonce, encryption_version, kms_key_id
        FROM email_generic_mailbox_bindings
        WHERE link_id = $1
        "#,
        link_id,
    )
    .fetch_optional(executor)
    .await?
    else {
        return Ok(None);
    };

    let protocol: MailboxProtocol = row
        .protocol
        .parse()
        .map_err(|_| InvalidConnection::Protocol)?;
    let server = match protocol {
        MailboxProtocol::Jmap => {
            let endpoint = row.endpoint.ok_or(InvalidConnection::Endpoint)?;
            MailboxServer::Jmap {
                endpoint: JmapOrigin::parse(&endpoint)?,
            }
        }
        MailboxProtocol::Imap => {
            let host = row.host.ok_or(InvalidConnection::Host)?;
            let port = row
                .port
                .and_then(|port| u16::try_from(port).ok())
                .and_then(NonZeroU16::new)
                .ok_or(InvalidConnection::Port)?;
            let tls: ImapTls = row
                .tls_mode
                .and_then(|mode| mode.parse().ok())
                .ok_or(InvalidConnection::TlsMode)?;
            MailboxServer::Imap {
                host: ImapHost::parse(&host)?,
                port,
                tls,
            }
        }
    };
    Ok(Some(StoredBinding {
        server,
        username: MailboxUsername::parse(&row.username)?,
        encrypted: EncryptedMailboxSecret {
            secret_ciphertext: row.secret_ciphertext,
            encrypted_data_key: row.encrypted_data_key,
            nonce: row.nonce,
            encryption_version: row.encryption_version,
            kms_key_id: row.kms_key_id,
        },
    }))
}
