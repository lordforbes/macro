//! Generic (JMAP and IMAP) mailbox credentials, kept by the authentication
//! service. Store them when a mailbox is connected; read them for each
//! sign-in and drop them straight after.

use generic_mailbox_credentials::credentials::MailboxCredentials;
use uuid::Uuid;

use crate::{
    AuthServiceClient,
    error::{AuthServiceClientError, GenericErrorResponse},
};

impl AuthServiceClient {
    /// Seal and store a generic link's credentials, replacing earlier ones.
    ///
    /// # Errors
    /// [`AuthServiceClientError::NotFound`] when the link is not a generic
    /// mailbox; a generic error when the service cannot store them now.
    pub async fn store_generic_mailbox_credentials(
        &self,
        link_id: Uuid,
        credentials: &MailboxCredentials,
    ) -> Result<(), AuthServiceClientError> {
        let response = self
            .client
            .put(self.generic_mailbox_url(link_id))
            .json(credentials)
            .send()
            .await
            .map_err(|_| unavailable())?;
        match response.status() {
            reqwest::StatusCode::NO_CONTENT => Ok(()),
            reqwest::StatusCode::NOT_FOUND => Err(AuthServiceClientError::NotFound),
            _ => Err(unavailable()),
        }
    }

    /// A generic link's credentials, secret included, for one sign-in.
    ///
    /// # Errors
    /// [`AuthServiceClientError::NotFound`] when the link has none stored.
    pub async fn generic_mailbox_credentials(
        &self,
        link_id: Uuid,
    ) -> Result<MailboxCredentials, AuthServiceClientError> {
        let response = self
            .client
            .get(self.generic_mailbox_url(link_id))
            .send()
            .await
            .map_err(|_| unavailable())?;
        match response.status() {
            reqwest::StatusCode::OK => response.json().await.map_err(|_| unavailable()),
            reqwest::StatusCode::NOT_FOUND => Err(AuthServiceClientError::NotFound),
            _ => Err(unavailable()),
        }
    }

    fn generic_mailbox_url(&self, link_id: Uuid) -> String {
        format!("{}/internal/generic_mailbox_credentials/{link_id}", self.url)
    }
}

fn unavailable() -> AuthServiceClientError {
    AuthServiceClientError::Generic(GenericErrorResponse {
        message: "Generic mailbox credentials are temporarily unavailable".into(),
    })
}
