//! Generic (JMAP and IMAP) mailboxes in the shared mailbox engine: their
//! credentials, and a gateway that routes each link to its provider's adapter.

#[cfg(test)]
mod test;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use authentication_service_client::{AuthServiceClient, error::AuthServiceClientError};
use email::domain::mailbox::projection::MailboxAttachmentAccess;
use email::domain::mailbox::{MailboxError, MailboxGateway, MailboxKey};
use email_api_client::domain::models::*;
use email_api_client::domain::ports::MailboxTokenSource;
use models_email::service::link::UserProvider;
use tracing::Instrument;
use uuid::Uuid;

/// A generic link's credentials, fetched from the authentication service for
/// each use and handed to the adapter as the token's opaque value. Never
/// cached here: the adapter drops them when the call ends.
#[derive(Clone)]
pub struct GenericCredentialsTokens(pub AuthServiceClient);

impl MailboxTokenSource for GenericCredentialsTokens {
    async fn access_token(
        &self,
        mailbox: MailboxAccess,
        _freshness: TokenFreshness,
    ) -> Result<AccessToken, TokenError> {
        let span = tracing::info_span!(
            "generic_mailbox.credentials",
            mailbox.link_id = %mailbox.link_id,
            outcome = tracing::field::Empty,
        );
        let result = self
            .0
            .generic_mailbox_credentials(mailbox.link_id)
            .instrument(span.clone())
            .await;
        let credentials = match result {
            Ok(credentials) => credentials,
            // No stored credentials: the user must connect the mailbox again.
            Err(AuthServiceClientError::NotFound) => {
                span.record("outcome", "missing");
                return Err(TokenError::ReauthRequired);
            }
            Err(_) => {
                span.record("outcome", "unavailable");
                return Err(TokenError::Transient {
                    message: "generic mailbox credentials are unavailable".into(),
                });
            }
        };
        span.record("outcome", "ok");
        serde_json::to_string(&credentials)
            .map(AccessToken::new)
            .map_err(|_| TokenError::Permanent {
                message: "generic mailbox credentials could not be encoded".into(),
            })
    }
}

/// Each link's provider, read once: a link's provider never changes.
#[derive(Clone)]
pub struct LinkProviders {
    db: sqlx::PgPool,
    known: Arc<Mutex<HashMap<Uuid, UserProvider>>>,
}

impl LinkProviders {
    pub fn new(db: sqlx::PgPool) -> Self {
        Self {
            db,
            known: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    async fn provider(&self, link_id: Uuid) -> Result<UserProvider, EmailApiError> {
        if let Some(provider) = self
            .known
            .lock()
            .ok()
            .and_then(|known| known.get(&link_id).copied())
        {
            return Ok(provider);
        }
        let stored = sqlx::query_scalar!(
            r#"SELECT provider::text AS "provider!" FROM email_links WHERE id = $1"#,
            link_id
        )
        .fetch_optional(&self.db)
        .await
        .map_err(|_| EmailApiError::Transient {
            message: "could not read the mailbox's provider".into(),
        })?
        .ok_or(EmailApiError::NotFound)?;
        let provider = match stored.as_str() {
            "GMAIL" => UserProvider::Gmail,
            "OUTLOOK" => UserProvider::Outlook,
            "JMAP" => UserProvider::Jmap,
            "IMAP" => UserProvider::Imap,
            _ => {
                return Err(EmailApiError::Permanent {
                    message: "unknown mailbox provider".into(),
                });
            }
        };
        if let Ok(mut known) = self.known.lock() {
            known.insert(link_id, provider);
        }
        Ok(provider)
    }
}

/// The adapter a link's provider uses.
enum Route {
    Outlook,
    Jmap,
}

/// Routes the mailbox engine's calls to the Outlook or JMAP gateway by the
/// link's provider. IMAP answers unsupported until its adapter exists; a
/// Gmail link never reaches the engine.
pub struct RoutedMailboxGateway<O, J> {
    pub outlook: O,
    pub jmap: J,
    pub providers: LinkProviders,
}

impl<O, J> RoutedMailboxGateway<O, J> {
    async fn route(&self, link_id: Uuid) -> Result<Route, EmailApiError> {
        match self.providers.provider(link_id).await? {
            UserProvider::Outlook => Ok(Route::Outlook),
            UserProvider::Jmap => Ok(Route::Jmap),
            UserProvider::Imap => Err(EmailApiError::Unsupported {
                capability: MailboxCapability::Read,
            }),
            UserProvider::Gmail => Err(EmailApiError::Permanent {
                message: "Gmail links are not synced by the mailbox engine".into(),
            }),
        }
    }
}

impl<O: MailboxGateway, J: MailboxGateway> MailboxGateway for RoutedMailboxGateway<O, J> {
    async fn folders(&self, mailbox: MailboxKey) -> Result<Vec<MailFolder>, EmailApiError> {
        match self.route(mailbox.link_id).await? {
            Route::Outlook => self.outlook.folders(mailbox).await,
            Route::Jmap => self.jmap.folders(mailbox).await,
        }
    }

    async fn changes(
        &self,
        mailbox: MailboxKey,
        folder: &ProviderId,
        position: Option<&StreamToken>,
    ) -> Result<MailboxChangePage, EmailApiError> {
        match self.route(mailbox.link_id).await? {
            Route::Outlook => self.outlook.changes(mailbox, folder, position).await,
            Route::Jmap => self.jmap.changes(mailbox, folder, position).await,
        }
    }

    async fn message(
        &self,
        mailbox: MailboxKey,
        id: &ProviderId,
        folders: &[MailFolder],
    ) -> Result<Option<MailboxMessage>, EmailApiError> {
        match self.route(mailbox.link_id).await? {
            Route::Outlook => self.outlook.message(mailbox, id, folders).await,
            Route::Jmap => self.jmap.message(mailbox, id, folders).await,
        }
    }

    async fn admitted_requests(&self, mailbox: MailboxKey) -> Option<u64> {
        match self.route(mailbox.link_id).await.ok()? {
            Route::Outlook => self.outlook.admitted_requests(mailbox).await,
            Route::Jmap => self.jmap.admitted_requests(mailbox).await,
        }
    }
}

impl<O: MailboxAttachmentAccess, J: MailboxAttachmentAccess> MailboxAttachmentAccess
    for RoutedMailboxGateway<O, J>
{
    async fn download(
        &self,
        mailbox: MailboxKey,
        message_id: &str,
        attachment_id: &str,
    ) -> Result<Vec<u8>, MailboxError> {
        match self
            .route(mailbox.link_id)
            .await
            .map_err(MailboxError::Provider)?
        {
            Route::Outlook => {
                self.outlook
                    .download(mailbox, message_id, attachment_id)
                    .await
            }
            Route::Jmap => self.jmap.download(mailbox, message_id, attachment_id).await,
        }
    }
}
