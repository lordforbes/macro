//! Vocabulary shared by the generic mailbox protocols (JMAP and IMAP):
//! stream cursors, folder roles and message flags.
//!
//! Everything here is pure mapping. Protocol clients live in the outbound
//! adapters; nothing in this module reads or writes a server.

#[cfg(test)]
mod test;
/// Threads synthesised from message headers.
pub mod threads;
/// The read-only safety check adapter tests run around a sync.
#[cfg(any(test, feature = "test-support"))]
pub mod safety;

use serde::{Deserialize, Serialize};

use super::{Attention, EmailApiError, FolderRole, MailboxState, StreamToken};

/// The cursor format this module writes. A token with any other version is
/// treated as outdated, so its stream is read again from the start.
const CURSOR_VERSION: u8 = 1;

/// Where a generic mailbox stream has read up to.
///
/// Stored as an opaque [`StreamToken`]; only this module reads its contents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "protocol", rename_all = "snake_case")]
pub enum GenericCursor {
    /// A JMAP folder is still being enumerated.
    JmapEnumerating {
        /// The account state read before enumeration began; changes after it
        /// are replayed once enumeration ends.
        state: String,
        /// The last message id of the previous page.
        after: String,
    },
    /// A JMAP stream has caught up to this account state.
    JmapState {
        /// The server's opaque state string.
        state: String,
    },
    /// An IMAP folder has been read up to a UID.
    Imap {
        /// The folder's UIDVALIDITY when the cursor was written. UIDs mean
        /// nothing under a different value.
        uidvalidity: u32,
        /// The highest UID already read; zero before the first message.
        last_uid: u32,
        /// The folder's HIGHESTMODSEQ, on servers with CONDSTORE.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        highestmodseq: Option<u64>,
    },
}

#[derive(Serialize, Deserialize)]
struct CursorEnvelope {
    version: u8,
    #[serde(flatten)]
    cursor: GenericCursor,
}

impl GenericCursor {
    /// Encode the cursor for storage.
    pub fn to_token(&self) -> Result<StreamToken, EmailApiError> {
        serde_json::to_string(&CursorEnvelope {
            version: CURSOR_VERSION,
            cursor: self.clone(),
        })
        .map(StreamToken::new)
        .map_err(|_| EmailApiError::Permanent {
            message: "could not encode a mailbox cursor".into(),
        })
    }

    /// Decode a stored cursor. A token this module did not write, or wrote in
    /// another version, is [`EmailApiError::OutdatedCursor`]: the caller
    /// resets the stream instead of guessing.
    pub fn from_token(token: &StreamToken) -> Result<Self, EmailApiError> {
        let envelope: CursorEnvelope =
            serde_json::from_str(token.expose()).map_err(|_| EmailApiError::OutdatedCursor)?;
        if envelope.version != CURSOR_VERSION {
            return Err(EmailApiError::OutdatedCursor);
        }
        Ok(envelope.cursor)
    }

    /// Check an IMAP cursor against the folder's current UIDVALIDITY. A
    /// different value means every stored UID is void.
    pub fn imap_position(&self, current_uidvalidity: u32) -> Result<u32, EmailApiError> {
        match self {
            Self::Imap {
                uidvalidity,
                last_uid,
                ..
            } if *uidvalidity == current_uidvalidity => Ok(*last_uid),
            _ => Err(EmailApiError::OutdatedCursor),
        }
    }
}

/// The role of a JMAP Mailbox from its `role` property (RFC 8621 section 2).
pub fn folder_role_from_jmap(role: Option<&str>) -> FolderRole {
    match role.map(str::to_ascii_lowercase).as_deref() {
        Some("inbox") => FolderRole::Inbox,
        Some("sent") => FolderRole::Sent,
        Some("drafts") => FolderRole::Drafts,
        Some("archive") => FolderRole::Archive,
        Some("trash") => FolderRole::Trash,
        Some("junk") => FolderRole::Junk,
        _ => FolderRole::Other,
    }
}

/// Whether an IMAP folder is a virtual view over other folders (`\All`,
/// `\Flagged`) or cannot hold messages (`\Noselect`, `\NonExistent`). Such a
/// folder is never synced: its messages are already read where they live.
pub fn imap_folder_is_not_synced<'a>(attributes: impl IntoIterator<Item = &'a str>) -> bool {
    attributes.into_iter().any(|attribute| {
        ["\\all", "\\flagged", "\\noselect", "\\nonexistent"]
            .contains(&attribute.to_ascii_lowercase().as_str())
    })
}

/// The role of an IMAP folder from its SPECIAL-USE attributes (RFC 6154),
/// falling back to the folder's own name (its last path segment) when the
/// server marks nothing. An ambiguous or unknown folder is
/// [`FolderRole::Other`].
pub fn folder_role_from_imap<'a>(
    attributes: impl IntoIterator<Item = &'a str>,
    leaf_name: &str,
) -> FolderRole {
    for attribute in attributes {
        let role = match attribute.to_ascii_lowercase().as_str() {
            "\\inbox" => FolderRole::Inbox,
            "\\sent" => FolderRole::Sent,
            "\\drafts" => FolderRole::Drafts,
            "\\archive" => FolderRole::Archive,
            "\\trash" => FolderRole::Trash,
            "\\junk" => FolderRole::Junk,
            _ => continue,
        };
        return role;
    }
    match leaf_name.trim().to_ascii_lowercase().as_str() {
        "inbox" => FolderRole::Inbox,
        "sent" | "sent items" | "sent messages" | "sent mail" => FolderRole::Sent,
        "drafts" | "draft" => FolderRole::Drafts,
        "archive" | "archives" => FolderRole::Archive,
        "trash" | "deleted items" | "deleted messages" | "bin" => FolderRole::Trash,
        "junk" | "spam" | "junk e-mail" | "junk email" | "bulk mail" => FolderRole::Junk,
        _ => FolderRole::Other,
    }
}

/// Message flags both protocols can express.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GenericFlags {
    /// `$seen` / `\Seen`.
    pub seen: bool,
    /// `$flagged` / `\Flagged`.
    pub flagged: bool,
    /// `$draft` / `\Draft`.
    pub draft: bool,
    /// `$answered` / `\Answered`.
    pub answered: bool,
    /// IMAP `\Deleted`: marked for removal, so treated as already gone.
    pub deleted: bool,
}

impl GenericFlags {
    /// Read JMAP keywords that are set (RFC 8621 section 4.1.1). Keywords are
    /// case-insensitive; unknown ones are ignored.
    pub fn from_jmap_keywords<'a>(keywords: impl IntoIterator<Item = &'a str>) -> Self {
        let mut flags = Self::default();
        for keyword in keywords {
            match keyword.to_ascii_lowercase().as_str() {
                "$seen" => flags.seen = true,
                "$flagged" => flags.flagged = true,
                "$draft" => flags.draft = true,
                "$answered" => flags.answered = true,
                _ => {}
            }
        }
        flags
    }

    /// Read IMAP system flags (RFC 3501 section 2.3.2). Keywords and unknown
    /// flags are ignored.
    pub fn from_imap_flags<'a>(flags: impl IntoIterator<Item = &'a str>) -> Self {
        let mut result = Self::default();
        for flag in flags {
            match flag.to_ascii_lowercase().as_str() {
                "\\seen" => result.seen = true,
                "\\flagged" => result.flagged = true,
                "\\draft" => result.draft = true,
                "\\answered" => result.answered = true,
                "\\deleted" => result.deleted = true,
                _ => {}
            }
        }
        result
    }

    /// The mailbox facts for a message with these flags in a folder of
    /// `role`. Neither protocol classifies attention.
    pub fn mailbox_state(self, role: FolderRole) -> MailboxState {
        MailboxState {
            is_read: self.seen,
            is_flagged: self.flagged,
            is_draft: self.draft || role == FolderRole::Drafts,
            in_inbox: role == FolderRole::Inbox,
            in_trash: role == FolderRole::Trash,
            in_junk: role == FolderRole::Junk,
            attention: Attention::Unknown,
        }
    }
}
