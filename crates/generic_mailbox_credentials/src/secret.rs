//! The secret a generic mailbox signs in with: a JMAP API key or an IMAP
//! password or app password.

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

/// The longest secret accepted. Far above any password or API key, and well
/// inside what one envelope should carry.
const MAX_SECRET_BYTES: usize = 4096;

/// A mailbox secret in plaintext.
///
/// Redacts its `Debug` and zeroizes on drop. Use it for one sign-in and drop
/// it; never log it, cache it or put it in an error. It serializes as the
/// plaintext only for the internal call that hands it to the email service.
#[derive(Deserialize)]
#[serde(try_from = "String")]
pub struct MailboxSecret(Zeroizing<String>);

impl MailboxSecret {
    /// Accept a secret exactly as supplied. Passwords may contain spaces, so
    /// nothing is trimmed; an empty secret, a NUL or an oversized value is
    /// refused.
    pub fn parse(supplied: String) -> Result<Self, MalformedMailboxSecret> {
        let supplied = Zeroizing::new(supplied);
        if supplied.is_empty() || supplied.len() > MAX_SECRET_BYTES || supplied.contains('\0') {
            return Err(MalformedMailboxSecret);
        }
        Ok(Self(supplied))
    }

    /// The plaintext. Only for the transport that signs in with it.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for MailboxSecret {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("MailboxSecret(redacted)")
    }
}

/// A supplied secret that cannot be stored. Says nothing about the value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the mailbox password or key is empty, too long or contains a NUL")]
pub struct MalformedMailboxSecret;

impl TryFrom<String> for MailboxSecret {
    type Error = MalformedMailboxSecret;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl Serialize for MailboxSecret {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.expose())
    }
}
