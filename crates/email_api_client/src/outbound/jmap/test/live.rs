//! A full read-only sync against a real, local JMAP server.
//!
//! Ignored by default. Run against a disposable local server only, never a
//! real mailbox (brief, safety rule 6):
//!
//! ```text
//! JMAP_LIVE_CREDENTIALS=/path/to/credentials.json \
//!   cargo test -p email_api_client --features outbound-jmap,test-support \
//!   --lib jmap::test::live -- --ignored --nocapture
//! ```
//!
//! `tooling/generic_mailbox/README.md` starts the server, provisions the
//! credentials file and seeds the fixtures this test expects.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use super::*;
use crate::domain::models::generic::safety::{
    MailboxSnapshot, ServerSnapshot, SnapshotSource, assert_read_only,
};
use crate::domain::models::{MailboxChangePage, MailboxMessage, ProviderId, StreamPosition};
use crate::domain::ports::{FolderChangeReader, MailboxContentReader};

#[derive(serde::Deserialize)]
struct LiveCredentials {
    origin: String,
    address: String,
    jmap_key: String,
}

mod live_env {
    macro_env_var::maybe_env_vars! {
        pub struct JmapLiveCredentials;
    }
}

fn live_token() -> AccessToken {
    let path = live_env::JmapLiveCredentials::new()
        .expect("set JMAP_LIVE_CREDENTIALS to a local test server's credentials file");
    let live: LiveCredentials =
        serde_json::from_str(&std::fs::read_to_string(path.as_ref()).unwrap()).unwrap();
    let credentials = MailboxCredentials {
        server: MailboxServer::Jmap {
            endpoint: JmapOrigin::parse(&live.origin).unwrap(),
        },
        username: MailboxUsername::parse(&live.address).unwrap(),
        secret: MailboxSecret::parse(live.jmap_key).unwrap(),
    };
    AccessToken::new(serde_json::to_string(&credentials).unwrap())
}

#[derive(serde::Deserialize)]
struct Keywords {
    id: String,
    #[serde(default)]
    keywords: HashMap<String, bool>,
}

/// Snapshots the server through a separate client instance.
struct JmapSnapshots {
    repository: JmapApiClientRepository,
    token: AccessToken,
}

impl SnapshotSource for JmapSnapshots {
    type Err = EmailApiError;

    async fn snapshot(&self) -> Result<ServerSnapshot, EmailApiError> {
        let jmap = self.repository.read_only(&self.token)?;
        let mut snapshot = ServerSnapshot::default();
        for mailbox in jmap.mailbox_get().await?.list {
            let mut messages = BTreeMap::new();
            let mut anchor: Option<String> = None;
            loop {
                let page = jmap
                    .email_query(&handle::EmailQuery {
                        in_mailbox: &mailbox.id,
                        before: None,
                        anchor: anchor.as_deref(),
                        limit: 200,
                    })
                    .await?;
                if page.ids.is_empty() {
                    break;
                }
                let found = jmap
                    .email_get::<Keywords>(&page.ids, &["id", "keywords"], None)
                    .await?;
                for email in found.list {
                    let set: BTreeSet<String> = email
                        .keywords
                        .into_iter()
                        .filter(|(_, on)| *on)
                        .map(|(keyword, _)| keyword)
                        .collect();
                    messages.insert(email.id, set);
                }
                if page.ids.len() < 200 {
                    break;
                }
                anchor = page.ids.last().cloned();
            }
            snapshot.mailboxes.insert(
                mailbox.id,
                MailboxSnapshot {
                    total: u64::from(mailbox.total_emails.unwrap_or(0)),
                    unread: u64::from(mailbox.unread_emails.unwrap_or(0)),
                    messages,
                },
            );
        }
        Ok(snapshot)
    }
}

/// Every message the sync read, by provider id.
struct SyncResult {
    messages: Vec<MailboxMessage>,
    second_round: Vec<MailboxChangePage>,
    attachment_bytes: Vec<usize>,
    mime_bytes: usize,
}

async fn full_sync(repository: &JmapApiClientRepository, token: &AccessToken) -> SyncResult {
    let link = uuid::Uuid::now_v7();
    let folders = repository.folders(token).await.unwrap();
    let mut messages = Vec::new();
    let mut checkpoints = Vec::new();
    for folder in &folders {
        let mut position = None;
        loop {
            let page = repository
                .folder_changes(token, &folder.id, position.as_ref())
                .await
                .unwrap();
            for id in &page.changed {
                if let Some(message) = repository.message(token, link, id, &folders).await.unwrap()
                {
                    messages.push(message);
                }
            }
            match page.position {
                StreamPosition::Continue(next) => position = Some(next),
                StreamPosition::Checkpoint(done) => {
                    checkpoints.push((folder.id.clone(), done));
                    break;
                }
            }
        }
    }
    let mut attachment_bytes = Vec::new();
    for message in &messages {
        let id = ProviderId::new(message.content.message.provider_id.clone().unwrap()).unwrap();
        for attachment in &message.content.message.attachments {
            let blob = ProviderId::new(attachment.provider_id.clone().unwrap()).unwrap();
            attachment_bytes.push(
                repository
                    .attachment(token, &id, &blob)
                    .await
                    .unwrap()
                    .len(),
            );
        }
    }
    let first = ProviderId::new(messages[0].content.message.provider_id.clone().unwrap()).unwrap();
    let mime_bytes = repository.mime(token, &first).await.unwrap().len();
    let mut second_round = Vec::new();
    for (folder, checkpoint) in &checkpoints {
        second_round.push(
            repository
                .folder_changes(token, folder, Some(checkpoint))
                .await
                .unwrap(),
        );
    }
    SyncResult {
        messages,
        second_round,
        attachment_bytes,
        mime_bytes,
    }
}

#[tokio::test]
#[ignore = "needs a local JMAP server seeded with the fixtures; see the module docs"]
async fn a_full_sync_reads_the_fixture_mailbox_and_changes_nothing() {
    let token = live_token();
    let source = JmapSnapshots {
        repository: JmapApiClientRepository::new().unwrap(),
        token: token.clone(),
    };
    let repository = JmapApiClientRepository::new().unwrap();

    let result = assert_read_only(&source, full_sync(&repository, &token))
        .await
        .unwrap();

    let subjects: Vec<&str> = result
        .messages
        .iter()
        .filter_map(|message| message.content.message.subject.as_deref())
        .collect();
    assert_eq!(result.messages.len(), 14, "{subjects:?}");
    assert!(subjects.contains(&"Café crème, naïve résumé — 日本語"));

    let offsite: BTreeSet<&str> = result
        .messages
        .iter()
        .filter(|message| {
            message
                .content
                .message
                .subject
                .as_deref()
                .is_some_and(|subject| subject.contains("offsite"))
        })
        .filter_map(|message| message.content.message.provider_thread_id.as_deref())
        .collect();
    assert_eq!(offsite.len(), 1, "the reply chain is one thread");

    let threads: BTreeSet<&str> = result
        .messages
        .iter()
        .filter_map(|message| message.content.message.provider_thread_id.as_deref())
        .collect();
    // 14 messages: one 6-message thread, two sharing a Message-ID, six alone.
    assert_eq!(threads.len(), 8, "{threads:?}");

    let mut sizes = result.attachment_bytes.clone();
    sizes.sort_unstable();
    assert_eq!(sizes, vec![14, 5 * 1024 * 1024]);
    assert!(result.mime_bytes > 0);
    assert!(
        result
            .second_round
            .iter()
            .all(|page| page.changed.is_empty() && page.removed.is_empty()),
        "nothing changed between rounds"
    );
}
