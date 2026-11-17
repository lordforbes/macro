//! Where a generic mailbox lives and who signs in to it.
//!
//! Endpoints, hosts and usernames identify a person's mailbox, so the `Debug`
//! output of every type here names the protocol only.

#[cfg(test)]
mod test;

use std::num::NonZeroU16;

use serde::{Deserialize, Serialize};
use url::Url;

/// The longest username accepted. An address is at most 320 characters.
const MAX_USERNAME_LENGTH: usize = 320;

/// The longest DNS name.
const MAX_HOST_LENGTH: usize = 253;

/// The mail protocol a generic mailbox speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::EnumString, strum::Display, strum::AsRefStr)]
#[strum(serialize_all = "snake_case")]
pub enum MailboxProtocol {
    /// JMAP (RFC 8620 and RFC 8621).
    Jmap,
    /// IMAP.
    Imap,
}

/// How an IMAP connection is encrypted. Plain-text connections are refused.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    strum::EnumString,
    strum::Display,
    strum::AsRefStr,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum ImapTls {
    /// TLS from the first byte, normally port 993.
    Implicit,
    /// A plain connection upgraded with STARTTLS before login.
    Starttls,
}

/// The server a generic mailbox is read from.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "protocol", rename_all = "snake_case")]
pub enum MailboxServer {
    /// A JMAP server, by the origin its session resource is discovered on.
    Jmap {
        /// The server origin.
        endpoint: JmapOrigin,
    },
    /// An IMAP server.
    Imap {
        /// The server's host name.
        host: ImapHost,
        /// The server's port.
        port: NonZeroU16,
        /// How the connection is encrypted.
        tls: ImapTls,
    },
}

impl MailboxServer {
    /// The protocol this server speaks.
    pub fn protocol(&self) -> MailboxProtocol {
        match self {
            Self::Jmap { .. } => MailboxProtocol::Jmap,
            Self::Imap { .. } => MailboxProtocol::Imap,
        }
    }
}

impl std::fmt::Debug for MailboxServer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MailboxServer")
            .field("protocol", &self.protocol())
            .finish_non_exhaustive()
    }
}

/// A JMAP server origin, such as `https://mail.example.com`.
///
/// HTTPS only, except plain HTTP to this machine or to a single-label host
/// (a container name such as `mailflare`, which public DNS never resolves),
/// for local test servers. The
/// adapter refuses a session whose API or download URLs leave this origin, so
/// the key is never sent anywhere else.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct JmapOrigin(String);

impl JmapOrigin {
    /// Accept an origin, or a URL whose origin is the server.
    pub fn parse(supplied: &str) -> Result<Self, InvalidConnection> {
        let url = Url::parse(supplied.trim()).map_err(|_| InvalidConnection::Endpoint)?;
        let host = url.host_str().ok_or(InvalidConnection::Endpoint)?;
        let secure = match url.scheme() {
            "https" => true,
            "http" => is_loopback(host) || is_single_label(host),
            _ => false,
        };
        if !secure || !url.username().is_empty() || url.password().is_some() {
            return Err(InvalidConnection::Endpoint);
        }
        Ok(Self(url.origin().ascii_serialization()))
    }

    /// The origin, without a trailing slash.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for JmapOrigin {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("JmapOrigin(redacted)")
    }
}

/// An IMAP server host name or address.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ImapHost(String);

impl ImapHost {
    /// Accept a host name, rejecting anything with a scheme, port or spaces.
    pub fn parse(supplied: &str) -> Result<Self, InvalidConnection> {
        let host = supplied.trim().to_ascii_lowercase();
        // A host is a URL host, and nothing else: no scheme, port, path or
        // credentials.
        let url = Url::parse(&format!("imaps://{host}")).map_err(|_| InvalidConnection::Host)?;
        let parsed_host = url.host_str().ok_or(InvalidConnection::Host)?;
        let exact = parsed_host == host
            && url.port().is_none()
            && url.path().is_empty()
            && url.username().is_empty();
        if host.is_empty() || host.len() > MAX_HOST_LENGTH || !exact {
            return Err(InvalidConnection::Host);
        }
        Ok(Self(host))
    }

    /// The host name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for ImapHost {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ImapHost(redacted)")
    }
}

/// The name a generic mailbox signs in with, usually its address.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct MailboxUsername(String);

impl MailboxUsername {
    /// Accept a username: not blank, no control characters, at most 320
    /// characters. Surrounding whitespace is removed.
    pub fn parse(supplied: &str) -> Result<Self, InvalidConnection> {
        let username = supplied.trim();
        let valid = !username.is_empty()
            && username.chars().count() <= MAX_USERNAME_LENGTH
            && !username.chars().any(char::is_control);
        if !valid {
            return Err(InvalidConnection::Username);
        }
        Ok(Self(username.to_owned()))
    }

    /// The username.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for MailboxUsername {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("MailboxUsername(redacted)")
    }
}

/// A connection setting that cannot be used. Carries no value, because the
/// value may identify a mailbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvalidConnection {
    /// The protocol is neither JMAP nor IMAP.
    #[error("the protocol must be jmap or imap")]
    Protocol,
    /// The JMAP endpoint is not an HTTPS origin (or HTTP to this machine or a
    /// single-label host).
    #[error("the JMAP endpoint must be an https:// address")]
    Endpoint,
    /// The IMAP host is not a bare host name.
    #[error("the IMAP host must be a host name without a scheme or port")]
    Host,
    /// A stored IMAP port is outside 1 to 65535.
    #[error("the IMAP port must be between 1 and 65535")]
    Port,
    /// The IMAP TLS mode is neither implicit TLS nor STARTTLS.
    #[error("the IMAP TLS mode must be implicit or starttls")]
    TlsMode,
    /// The username is blank, too long or contains control characters.
    #[error("the username is not valid")]
    Username,
}

fn is_loopback(host: &str) -> bool {
    host == "localhost"
        || host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

/// A host name with no dots: a name on a local or container network. An IP
/// address is not one, so `http://10.0.0.1` stays refused.
fn is_single_label(host: &str) -> bool {
    !host.is_empty()
        && !host.contains(['.', ':', '[', ']'])
        && host.parse::<std::net::IpAddr>().is_err()
}

impl TryFrom<String> for JmapOrigin {
    type Error = InvalidConnection;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<JmapOrigin> for String {
    fn from(value: JmapOrigin) -> Self {
        value.0
    }
}

impl TryFrom<String> for ImapHost {
    type Error = InvalidConnection;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<ImapHost> for String {
    fn from(value: ImapHost) -> Self {
        value.0
    }
}

impl TryFrom<String> for MailboxUsername {
    type Error = InvalidConnection;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<MailboxUsername> for String {
    fn from(value: MailboxUsername) -> Self {
        value.0
    }
}
