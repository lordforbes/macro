//! Generic mailbox credentials, kept by the authentication service.

use authentication_service_client::{AuthServiceClient, error::AuthServiceClientError};
use email::domain::mailbox::generic_connection::GenericCredentialStore;
use generic_mailbox_credentials::credentials::MailboxCredentials;
use uuid::Uuid;

/// Stores credentials through the authentication service's internal route.
pub struct AuthGenericCredentials(pub AuthServiceClient);

impl GenericCredentialStore for AuthGenericCredentials {
    type Err = AuthServiceClientError;

    async fn store(
        &self,
        link_id: Uuid,
        credentials: &MailboxCredentials,
    ) -> Result<(), AuthServiceClientError> {
        self.0
            .store_generic_mailbox_credentials(link_id, credentials)
            .await
    }
}
