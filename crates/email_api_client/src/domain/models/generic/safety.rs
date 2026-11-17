//! The read-only safety check for generic mailbox adapters: snapshot a
//! server's state, run a sync, snapshot again, and fail on any difference.
//!
//! A snapshot records, per mailbox, the server's message and unread counts
//! and every message's id with its flags or keywords. JMAP sources record
//! email ids and keywords; IMAP sources record `UIDVALIDITY:UID` and flags,
//! including `\Seen` and `\Recent`. Sources are test code with their own
//! connection: they must not use the adapter under test, and they must read
//! without changing anything themselves (JMAP `Email/get`; IMAP `EXAMINE` and
//! `UID FETCH (FLAGS)`).

#[cfg(test)]
mod test;

use std::collections::{BTreeMap, BTreeSet};

/// One mailbox (folder) as the server reports it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MailboxSnapshot {
    /// The server's message count for the mailbox.
    pub total: u64,
    /// The server's unread count for the mailbox.
    pub unread: u64,
    /// Every message's id, with its flags or keywords.
    pub messages: BTreeMap<String, BTreeSet<String>>,
}

/// A server's state, by mailbox id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServerSnapshot {
    /// Every mailbox, by its id.
    pub mailboxes: BTreeMap<String, MailboxSnapshot>,
}

/// One difference between two snapshots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotChange {
    /// A mailbox appeared.
    MailboxAdded {
        /// The mailbox id.
        mailbox: String,
    },
    /// A mailbox disappeared.
    MailboxRemoved {
        /// The mailbox id.
        mailbox: String,
    },
    /// The server's counts for a mailbox changed.
    CountsChanged {
        /// The mailbox id.
        mailbox: String,
        /// Total and unread before.
        before: (u64, u64),
        /// Total and unread after.
        after: (u64, u64),
    },
    /// A message appeared in a mailbox.
    MessageAdded {
        /// The mailbox id.
        mailbox: String,
        /// The message id.
        message: String,
    },
    /// A message left a mailbox (moved, deleted or expunged).
    MessageRemoved {
        /// The mailbox id.
        mailbox: String,
        /// The message id.
        message: String,
    },
    /// A message's flags or keywords changed.
    FlagsChanged {
        /// The mailbox id.
        mailbox: String,
        /// The message id.
        message: String,
        /// Flags set by the run.
        added: BTreeSet<String>,
        /// Flags cleared by the run.
        removed: BTreeSet<String>,
    },
}

/// Everything that differs between `before` and `after`, in mailbox and
/// message order. Empty when the server is unchanged.
pub fn changes(before: &ServerSnapshot, after: &ServerSnapshot) -> Vec<SnapshotChange> {
    let mut changes = Vec::new();
    for (mailbox, old) in &before.mailboxes {
        let Some(new) = after.mailboxes.get(mailbox) else {
            changes.push(SnapshotChange::MailboxRemoved {
                mailbox: mailbox.clone(),
            });
            continue;
        };
        if (old.total, old.unread) != (new.total, new.unread) {
            changes.push(SnapshotChange::CountsChanged {
                mailbox: mailbox.clone(),
                before: (old.total, old.unread),
                after: (new.total, new.unread),
            });
        }
        message_changes(mailbox, old, new, &mut changes);
    }
    for mailbox in after.mailboxes.keys() {
        if !before.mailboxes.contains_key(mailbox) {
            changes.push(SnapshotChange::MailboxAdded {
                mailbox: mailbox.clone(),
            });
        }
    }
    changes
}

fn message_changes(
    mailbox: &str,
    old: &MailboxSnapshot,
    new: &MailboxSnapshot,
    changes: &mut Vec<SnapshotChange>,
) {
    for (message, old_flags) in &old.messages {
        let Some(new_flags) = new.messages.get(message) else {
            changes.push(SnapshotChange::MessageRemoved {
                mailbox: mailbox.to_owned(),
                message: message.clone(),
            });
            continue;
        };
        if old_flags != new_flags {
            changes.push(SnapshotChange::FlagsChanged {
                mailbox: mailbox.to_owned(),
                message: message.clone(),
                added: new_flags.difference(old_flags).cloned().collect(),
                removed: old_flags.difference(new_flags).cloned().collect(),
            });
        }
    }
    for message in new.messages.keys() {
        if !old.messages.contains_key(message) {
            changes.push(SnapshotChange::MessageAdded {
                mailbox: mailbox.to_owned(),
                message: message.clone(),
            });
        }
    }
}

/// Reads a server's state through a connection of its own.
pub trait SnapshotSource: Send + Sync {
    /// The failure to read.
    type Err: std::error::Error + Send + Sync + 'static;

    /// The server's current state.
    fn snapshot(&self) -> impl Future<Output = Result<ServerSnapshot, Self::Err>> + Send;
}

/// Why the read-only check failed.
#[derive(Debug, thiserror::Error)]
pub enum ReadOnlyCheckError<E: std::error::Error + 'static> {
    /// A snapshot could not be read.
    #[error("could not snapshot the server")]
    Snapshot(#[source] E),
    /// The run changed the server.
    #[error("the run changed the server: {0:?}")]
    Changed(Vec<SnapshotChange>),
}

/// Run `sync` between two snapshots of `source` and return its output, or
/// every change it made to the server.
///
/// # Errors
/// [`ReadOnlyCheckError::Changed`] when the server differs afterwards.
pub async fn assert_read_only<S, T>(
    source: &S,
    sync: impl Future<Output = T>,
) -> Result<T, ReadOnlyCheckError<S::Err>>
where
    S: SnapshotSource,
{
    let before = source
        .snapshot()
        .await
        .map_err(ReadOnlyCheckError::Snapshot)?;
    let output = sync.await;
    let after = source
        .snapshot()
        .await
        .map_err(ReadOnlyCheckError::Snapshot)?;
    let changes = changes(&before, &after);
    if changes.is_empty() {
        Ok(output)
    } else {
        Err(ReadOnlyCheckError::Changed(changes))
    }
}
