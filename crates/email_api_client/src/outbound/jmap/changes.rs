//! Folder change streams from `Email/query` and `Email/changes`.
//!
//! JMAP reports changes for the whole account, not per mailbox, so every
//! folder stream reads the same change log and keeps what concerns it:
//!
//! * First round: read the account's Email state, then enumerate the folder
//!   newest first, paging by anchor. The state read first becomes the round's
//!   checkpoint, so anything that changes during enumeration is replayed.
//! * Later rounds: `Email/changes` from the checkpoint. An email now in this
//!   folder is changed. A destroyed email, or one now elsewhere, is removed
//!   from this stream; the engine reads its current state to settle it.
//!
//! A server that cannot calculate changes from a state answers
//! `cannotCalculateChanges`, which becomes `OutdatedCursor`, and the engine
//! enumerates the folder again.
//!
//! Known limit: Mailflare orders only the newest 10,000 matches of a query, so
//! a larger folder needs date-windowed enumeration, which is not built yet.

use super::JmapApiClientRepository;
use super::handle::EmailQuery;
use crate::domain::models::generic::GenericCursor;
use crate::domain::models::{
    AccessToken, EmailApiError, MailboxChangePage, ProviderId, StreamPosition, StreamToken,
};
use crate::domain::ports::FolderChangeReader;

/// Ids per page: within Mailflare's query and get limits.
const PAGE: u32 = 200;

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Placement {
    id: String,
    #[serde(default)]
    mailbox_ids: std::collections::HashMap<String, bool>,
}

fn ids(values: Vec<String>) -> Result<Vec<ProviderId>, EmailApiError> {
    values.into_iter().map(ProviderId::new).collect()
}

impl JmapApiClientRepository {
    async fn enumerate(
        &self,
        token: &AccessToken,
        folder: &ProviderId,
        resume: Option<(String, String)>,
    ) -> Result<MailboxChangePage, EmailApiError> {
        let jmap = self.read_only(token)?;
        let (state, after) = match resume {
            Some((state, after)) => (state, Some(after)),
            None => {
                // An Email/get of no ids reads only the current state.
                let state = jmap.email_get::<Placement>(&[], &["id"], None).await?.state;
                (state, None)
            }
        };
        let page = jmap
            .email_query(&EmailQuery {
                in_mailbox: folder.as_str(),
                before: None,
                anchor: after.as_deref(),
                limit: PAGE,
            })
            .await?;
        let full_page = page.ids.len() >= PAGE as usize;
        let cursor = match page.ids.last() {
            Some(last) if full_page => GenericCursor::JmapEnumerating {
                state,
                after: last.clone(),
            },
            _ => GenericCursor::JmapState { state },
        };
        let token = cursor.to_token()?;
        Ok(MailboxChangePage {
            changed: ids(page.ids)?,
            removed: Vec::new(),
            position: match cursor {
                GenericCursor::JmapEnumerating { .. } => StreamPosition::Continue(token),
                _ => StreamPosition::Checkpoint(token),
            },
            reader_revision: Self::READER_REVISION,
        })
    }

    async fn changes(
        &self,
        token: &AccessToken,
        folder: &ProviderId,
        since: &str,
    ) -> Result<MailboxChangePage, EmailApiError> {
        let jmap = self.read_only(token)?;
        let changes = jmap.email_changes(since, PAGE).await?;
        let mut touched = changes.created;
        touched.extend(changes.updated);
        let mut removed = changes.destroyed;
        let mut changed = Vec::new();
        // Where each touched email is now decides which side of the page it is on.
        if !touched.is_empty() {
            let placed = jmap
                .email_get::<Placement>(&touched, &["id", "mailboxIds"], None)
                .await?;
            for email in placed.list {
                let here = email
                    .mailbox_ids
                    .get(folder.as_str())
                    .copied()
                    .unwrap_or(false);
                if here {
                    changed.push(email.id);
                } else {
                    removed.push(email.id);
                }
            }
            // Destroyed after the change log was read.
            removed.extend(placed.not_found);
        }
        let token = GenericCursor::JmapState {
            state: changes.new_state,
        }
        .to_token()?;
        Ok(MailboxChangePage {
            changed: ids(changed)?,
            removed: ids(removed)?,
            position: if changes.has_more_changes {
                StreamPosition::Continue(token)
            } else {
                StreamPosition::Checkpoint(token)
            },
            reader_revision: Self::READER_REVISION,
        })
    }
}

impl FolderChangeReader for JmapApiClientRepository {
    const READER_REVISION: i16 = 1;

    async fn folder_changes(
        &self,
        token: &AccessToken,
        folder: &ProviderId,
        position: Option<&StreamToken>,
    ) -> Result<MailboxChangePage, EmailApiError> {
        let Some(position) = position else {
            return self.enumerate(token, folder, None).await;
        };
        match GenericCursor::from_token(position)? {
            GenericCursor::JmapEnumerating { state, after } => {
                self.enumerate(token, folder, Some((state, after))).await
            }
            GenericCursor::JmapState { state } => self.changes(token, folder, &state).await,
            // An IMAP cursor on a JMAP stream cannot be continued.
            GenericCursor::Imap { .. } => Err(EmailApiError::OutdatedCursor),
        }
    }
}
