//! A generic mailbox's full sign-in: where, who and the secret.
//!
//! The body of the authentication service's internal store and read routes.

use serde::{Deserialize, Serialize};

use crate::{
    connection::{MailboxServer, MailboxUsername},
    secret::MailboxSecret,
};

/// Everything needed to sign in to a generic mailbox. `Debug` names the
/// protocol only.
#[derive(Debug, Serialize, Deserialize)]
pub struct MailboxCredentials {
    /// The server the mailbox is read from.
    pub server: MailboxServer,
    /// The name it signs in with.
    pub username: MailboxUsername,
    /// The password or API key.
    pub secret: MailboxSecret,
}
