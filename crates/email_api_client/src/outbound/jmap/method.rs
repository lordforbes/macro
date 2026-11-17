//! The JMAP methods this adapter can send: the read-only set the generic
//! mailbox brief allows, and nothing else.
//!
//! The transport only accepts a [`ReadMethod`], so a write method (`Email/set`,
//! `Mailbox/set`, `EmailSubmission/set`, ...) cannot be expressed anywhere in
//! the adapter. A phase that is allowed to write adds its methods here,
//! deliberately, and changes the allowlist test with them.

#[cfg(test)]
mod test;

use serde_json::Value;

/// Capabilities a call may need in `using`. Only these are ever declared,
/// whatever else the server's session advertises.
pub(super) const CORE: &str = "urn:ietf:params:jmap:core";
/// The mail capability (RFC 8621).
pub(super) const MAIL: &str = "urn:ietf:params:jmap:mail";

/// A read-only JMAP method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::EnumIter)]
pub(super) enum ReadMethod {
    CoreEcho,
    MailboxGet,
    MailboxChanges,
    EmailQuery,
    EmailGet,
    EmailChanges,
    ThreadGet,
}

impl ReadMethod {
    /// The method's wire name.
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::CoreEcho => "Core/echo",
            Self::MailboxGet => "Mailbox/get",
            Self::MailboxChanges => "Mailbox/changes",
            Self::EmailQuery => "Email/query",
            Self::EmailGet => "Email/get",
            Self::EmailChanges => "Email/changes",
            Self::ThreadGet => "Thread/get",
        }
    }

    /// Whether the call is scoped to an account (everything but `Core/echo`).
    pub(super) fn takes_account(self) -> bool {
        self != Self::CoreEcho
    }

    /// The capability the method belongs to.
    pub(super) fn capability(self) -> &'static str {
        match self {
            Self::CoreEcho => CORE,
            _ => MAIL,
        }
    }
}

/// One call: a read-only method and its arguments, without `accountId`, which
/// the transport fills in from the session.
#[derive(Debug, Clone)]
pub(super) struct ReadCall {
    pub method: ReadMethod,
    pub arguments: serde_json::Map<String, Value>,
}

impl ReadCall {
    pub(super) fn new(method: ReadMethod, arguments: Value) -> Self {
        let arguments = match arguments {
            Value::Object(map) => map,
            _ => serde_json::Map::new(),
        };
        Self { method, arguments }
    }
}
