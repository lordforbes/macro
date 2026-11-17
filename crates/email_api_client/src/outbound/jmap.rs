//! Read-only JMAP (RFC 8620 core, RFC 8621 mail) adapter for generic mailboxes.
//!
//! One adapter serves every JMAP link. Each call's [`AccessToken`] carries the
//! link's credentials (server origin and API key, serialized
//! [`MailboxCredentials`] from the authentication service), so the key is only
//! ever sent to that link's own server. Every request goes through
//! [`handle::ReadOnlyJmap`], which can express only the read-only methods the
//! brief allows (see `method.rs`). Written against Mailflare and kept generic.

mod handle;
mod method;
mod transport;

#[cfg(test)]
mod test;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use generic_mailbox_credentials::{
    connection::MailboxServer, credentials::MailboxCredentials, secret::MailboxSecret,
};
use reqwest::Client;
use sha2::{Digest, Sha256};
use url::Url;

use crate::domain::models::{AccessToken, EmailApiError, MailboxAccess};
use crate::domain::ports::{MailboxRequestCount, ScopedMailboxRepository};

pub use handle::ReadOnlyJmap;

/// A request is abandoned after this long; a server that slow is retried.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// The adapter. Holds no credential or mailbox identity of its own.
#[derive(Clone)]
pub struct JmapApiClientRepository {
    client: Client,
    /// Discovered sessions, by a hash of the credentials that opened them.
    sessions: Arc<Mutex<HashMap<String, transport::Session>>>,
}

/// A JMAP link's server origin and API key, taken from one call's token.
pub(crate) struct JmapCredentials {
    origin: Url,
    secret: MailboxSecret,
}

impl JmapApiClientRepository {
    /// Build the adapter. Redirects and automatic retries are off, so a key is
    /// never replayed to another host.
    pub fn new() -> Result<Self, EmailApiError> {
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .timeout(REQUEST_TIMEOUT)
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .map_err(|_| EmailApiError::Permanent {
                message: "could not initialize the JMAP client".into(),
            })?;
        Ok(Self {
            client,
            sessions: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// The read-only operations, for the link whose credentials `token` holds.
    pub fn read_only(&self, token: &AccessToken) -> Result<ReadOnlyJmap<'_>, EmailApiError> {
        Ok(ReadOnlyJmap::new(self, credentials(token)?))
    }

    fn cached_session(&self, credentials: &JmapCredentials) -> Option<transport::Session> {
        let sessions = self.sessions.lock().ok()?;
        sessions.get(&session_key(credentials)).cloned()
    }

    fn cache_session(&self, credentials: &JmapCredentials, session: transport::Session) {
        if let Ok(mut sessions) = self.sessions.lock() {
            sessions.insert(session_key(credentials), session);
        }
    }

    fn forget_session(&self, credentials: &JmapCredentials) {
        if let Ok(mut sessions) = self.sessions.lock() {
            sessions.remove(&session_key(credentials));
        }
    }
}

/// Parse the credentials a generic token source put in `token`.
fn credentials(token: &AccessToken) -> Result<JmapCredentials, EmailApiError> {
    let credentials: MailboxCredentials =
        serde_json::from_str(token.expose_secret()).map_err(|_| EmailApiError::Permanent {
            message: "JMAP credentials are malformed".into(),
        })?;
    let MailboxServer::Jmap { endpoint } = credentials.server else {
        return Err(EmailApiError::Permanent {
            message: "credentials are not for a JMAP server".into(),
        });
    };
    let origin = Url::parse(endpoint.as_str()).map_err(|_| EmailApiError::Permanent {
        message: "JMAP origin is malformed".into(),
    })?;
    Ok(JmapCredentials {
        origin,
        secret: credentials.secret,
    })
}

/// Sessions are keyed by a hash, so the cache never holds the key itself.
fn session_key(credentials: &JmapCredentials) -> String {
    let mut hasher = Sha256::new();
    hasher.update(credentials.origin.as_str().as_bytes());
    hasher.update([0]);
    hasher.update(credentials.secret.expose().as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

impl MailboxRequestCount for JmapApiClientRepository {
    /// No request gate is attached, so no count is known.
    async fn admitted_requests(&self, _: MailboxAccess) -> Option<u64> {
        None
    }
}

impl ScopedMailboxRepository for JmapApiClientRepository {
    fn for_mailbox(&self, _: MailboxAccess) -> Self {
        self.clone()
    }
}
