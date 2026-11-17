//! Folders, message snapshots and blobs, read through the read-only handle.

#[cfg(test)]
mod test;

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use models_email::service::{address::ContactInfo, attachment::Attachment, message::Message};
use serde::Deserialize;
use uuid::Uuid;

use super::JmapApiClientRepository;
use super::handle::{BodyOptions, EmailQuery, WireMailbox};
use crate::domain::models::generic::{
    GenericFlags, folder_role_from_jmap,
    threads::{SyntheticThreads, ThreadHeaders},
};
use crate::domain::models::{
    AccessToken, EmailApiError, FolderRole, MailFolder, MailboxMessage, MailboxOrganization,
    MessageWithCalendarParts, ProviderId,
};
use crate::domain::ports::MailboxContentReader;

/// Body values larger than this are cut by the server, so a huge message
/// cannot be pulled into memory whole.
const MAX_BODY_VALUE_BYTES: u32 = 2 * 1024 * 1024;

const ORGANIZATION_PROPERTIES: &[&str] = &["id", "mailboxIds", "keywords"];

/// Enough to show who wrote and what about, without bodies.
const SUMMARY_PROPERTIES: &[&str] = &[
    "id",
    "mailboxIds",
    "keywords",
    "messageId",
    "inReplyTo",
    "references",
    "from",
    "to",
    "cc",
    "subject",
    "receivedAt",
    "sentAt",
    "preview",
];

const MESSAGE_PROPERTIES: &[&str] = &[
    "id",
    "blobId",
    "mailboxIds",
    "keywords",
    "size",
    "receivedAt",
    "sentAt",
    "messageId",
    "inReplyTo",
    "references",
    "from",
    "to",
    "cc",
    "bcc",
    "subject",
    "preview",
    "textBody",
    "htmlBody",
    "bodyValues",
    "attachments",
];

#[derive(Deserialize)]
struct WireAddress {
    name: Option<String>,
    email: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WirePart {
    part_id: Option<String>,
    blob_id: Option<String>,
    size: Option<i64>,
    name: Option<String>,
    #[serde(rename = "type")]
    content_type: Option<String>,
    cid: Option<String>,
}

#[derive(Deserialize)]
struct WireBodyValue {
    value: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct WireEmail {
    id: String,
    #[serde(default)]
    blob_id: Option<String>,
    #[serde(default)]
    mailbox_ids: HashMap<String, bool>,
    #[serde(default)]
    keywords: HashMap<String, bool>,
    #[serde(default)]
    size: Option<i64>,
    #[serde(default)]
    received_at: Option<DateTime<Utc>>,
    #[serde(default)]
    sent_at: Option<DateTime<Utc>>,
    #[serde(default)]
    message_id: Option<Vec<String>>,
    #[serde(default)]
    in_reply_to: Option<Vec<String>>,
    #[serde(default)]
    references: Option<Vec<String>>,
    #[serde(default)]
    from: Option<Vec<WireAddress>>,
    #[serde(default)]
    to: Option<Vec<WireAddress>>,
    #[serde(default)]
    cc: Option<Vec<WireAddress>>,
    #[serde(default)]
    bcc: Option<Vec<WireAddress>>,
    #[serde(default)]
    subject: Option<String>,
    #[serde(default)]
    preview: Option<String>,
    #[serde(default)]
    text_body: Vec<WirePart>,
    #[serde(default)]
    html_body: Vec<WirePart>,
    #[serde(default)]
    body_values: HashMap<String, WireBodyValue>,
    #[serde(default)]
    attachments: Vec<WirePart>,
}

/// The folders to sync: every mailbox except account containers. A
/// container (Mailflare's top-level mailbox per address) has no role and
/// holds role mailboxes; no email lists it, and its counts repeat its
/// children's, so syncing it would read every message twice.
pub(super) fn folders_from(mailboxes: Vec<WireMailbox>) -> Result<Vec<MailFolder>, EmailApiError> {
    let parents_of_roles: HashSet<&str> = mailboxes
        .iter()
        .filter(|mailbox| mailbox.role.is_some())
        .filter_map(|mailbox| mailbox.parent_id.as_deref())
        .collect();
    let containers: HashSet<String> = mailboxes
        .iter()
        .filter(|mailbox| mailbox.role.is_none() && parents_of_roles.contains(mailbox.id.as_str()))
        .map(|mailbox| mailbox.id.clone())
        .collect();
    let parents: HashSet<&str> = mailboxes
        .iter()
        .filter_map(|mailbox| mailbox.parent_id.as_deref())
        .collect();

    mailboxes
        .iter()
        .filter(|mailbox| !containers.contains(&mailbox.id))
        .map(|mailbox| {
            // A role mailbox under a container is shown at the top level.
            let parent_id = mailbox
                .parent_id
                .as_ref()
                .filter(|parent| !containers.contains(*parent))
                .map(ProviderId::new)
                .transpose()?;
            Ok(MailFolder {
                id: ProviderId::new(mailbox.id.clone())?,
                parent_id,
                name: mailbox.name.clone(),
                role: folder_role_from_jmap(mailbox.role.as_deref()),
                has_children: parents.contains(mailbox.id.as_str()),
                total_items: mailbox.total_emails,
            })
        })
        .collect()
}

fn contacts(addresses: Option<Vec<WireAddress>>) -> Vec<ContactInfo> {
    addresses
        .unwrap_or_default()
        .into_iter()
        .filter_map(|address| {
            let email = address.email.filter(|email| !email.is_empty())?;
            Some(ContactInfo {
                email,
                name: address.name.filter(|name| !name.is_empty()),
                photo_url: None,
            })
        })
        .collect()
}

/// The decoded body values of one media type, joined.
fn body(parts: &[WirePart], values: &HashMap<String, WireBodyValue>, mime: &str) -> Option<String> {
    let text: Vec<&str> = parts
        .iter()
        .filter(|part| {
            part.content_type
                .as_deref()
                .is_some_and(|kind| kind.eq_ignore_ascii_case(mime))
        })
        .filter_map(|part| part.part_id.as_ref())
        .filter_map(|id| values.get(id))
        .map(|value| value.value.as_str())
        .collect();
    (!text.is_empty()).then(|| text.join("\n"))
}

fn bracketed(id: &str) -> String {
    format!("<{}>", id.trim_matches(|c| c == '<' || c == '>'))
}

/// A list of ids as a header value (`<a> <b>`).
fn id_header(ids: Option<&Vec<String>>) -> Option<String> {
    let ids = ids.filter(|ids| !ids.is_empty())?;
    Some(
        ids.iter()
            .map(|id| bracketed(id))
            .collect::<Vec<_>>()
            .join(" "),
    )
}

/// Message-ID style headers in the name/value shape ingestion reads.
fn headers_json(
    message_id: Option<&str>,
    in_reply_to: Option<&str>,
    references: Option<&str>,
) -> serde_json::Value {
    let headers: Vec<serde_json::Value> = [
        ("Message-ID", message_id),
        ("In-Reply-To", in_reply_to),
        ("References", references),
    ]
    .into_iter()
    .filter_map(|(name, value)| {
        value.map(|value| serde_json::json!({"name": name, "value": value}))
    })
    .collect();
    serde_json::Value::Array(headers)
}

/// The message's folder: its one mailbox, or the smallest id when a server
/// files it in several (Mailflare never does). Containers are not folders.
fn folder_of(
    source: &WireEmail,
    folders: &[MailFolder],
) -> Result<Option<ProviderId>, EmailApiError> {
    source
        .mailbox_ids
        .iter()
        .filter(|(id, present)| {
            **present
                && folders
                    .iter()
                    .any(|folder| folder.id.as_str() == id.as_str())
        })
        .map(|(id, _)| id.as_str())
        .min()
        .map(ProviderId::new)
        .transpose()
}

pub(super) fn normalize(
    source: WireEmail,
    link_id: Uuid,
    folders: &[MailFolder],
) -> Result<MailboxMessage, EmailApiError> {
    ProviderId::new(source.id.clone())?;
    let folder_id = folder_of(&source, folders)?;
    let role = MailFolder::effective_role(folders, folder_id.as_ref());
    let flags = GenericFlags::from_jmap_keywords(
        source
            .keywords
            .iter()
            .filter(|(_, set)| **set)
            .map(|(keyword, _)| keyword.as_str()),
    );
    let state = flags.mailbox_state(role);

    let message_id = id_header(source.message_id.as_ref());
    let in_reply_to = id_header(source.in_reply_to.as_ref());
    let references = id_header(source.references.as_ref());
    // Threads come from headers, not the server's threadId: Mailflare names
    // a thread at arrival, so a reply that arrives before its parent starts a
    // thread of its own and the two are never joined. Headers give the same
    // answer in any arrival order, and the same answer IMAP gets.
    let thread_id = SyntheticThreads::new()
        .insert(
            &source.id,
            ThreadHeaders {
                message_id: message_id.as_deref(),
                in_reply_to: in_reply_to.as_deref(),
                references: references.as_deref(),
                subject: source.subject.as_deref(),
            },
        )
        .thread_id;

    let html = body(&source.html_body, &source.body_values, "text/html");
    let (body_text, body_html_sanitized) = match html {
        Some(html) => {
            let sanitized = email_utils::sanitize_email_html(&html);
            (
                email_utils::body_parsed::html_to_plaintext(&sanitized),
                Some(sanitized),
            )
        }
        None => (
            body(&source.text_body, &source.body_values, "text/plain"),
            None,
        ),
    };
    let snippet = email_utils::body_replyless::reply_snippet(
        source.subject.as_deref(),
        body_html_sanitized.as_deref(),
        body_text.as_deref(),
    )
    .or(source.preview);
    let attachments: Vec<Attachment> = source
        .attachments
        .into_iter()
        .filter(|part| part.blob_id.is_some())
        .map(|part| Attachment {
            reference_url: None,
            db_id: Uuid::now_v7(),
            provider_id: part.blob_id,
            data_url: None,
            filename: part.name,
            mime_type: part.content_type,
            size_bytes: part.size,
            sfs_id: None,
            content_id: part.cid,
        })
        .collect();

    let now = Utc::now();
    let message = Message {
        db_id: Uuid::now_v7(),
        provider_id: Some(source.id),
        thread_db_id: Uuid::now_v7(),
        provider_thread_id: Some(thread_id.as_str().to_owned()),
        replying_to_id: None,
        global_id: message_id
            .as_deref()
            .and_then(|ids| ids.split_whitespace().next())
            .map(str::to_owned),
        link_id,
        subject: source.subject,
        snippet,
        provider_history_id: None,
        internal_date_ts: source.received_at.or(source.sent_at),
        sent_at: source.sent_at,
        size_estimate: source.size,
        is_read: state.is_read,
        is_starred: state.is_flagged,
        // JMAP has no provenance flag; the Sent role is the evidence.
        is_sent: role == FolderRole::Sent,
        is_draft: state.is_draft,
        scheduled_send_time: None,
        has_attachments: !attachments.is_empty(),
        from: contacts(source.from).into_iter().next(),
        to: contacts(source.to),
        cc: contacts(source.cc),
        bcc: contacts(source.bcc),
        labels: Vec::new(),
        body_text,
        body_html_sanitized,
        body_macro: None,
        attachments,
        attachments_draft: Vec::new(),
        attachments_forwarded: Vec::new(),
        headers_json: Some(headers_json(
            message_id.as_deref(),
            in_reply_to.as_deref(),
            references.as_deref(),
        )),
        created_at: source.received_at.unwrap_or(now),
        updated_at: now,
    };
    Ok(MailboxMessage {
        draft_correlation: None,
        content: MessageWithCalendarParts {
            message,
            calendar_parts: Vec::new(),
        },
        calendar_parts_unavailable: false,
        state,
        folder_id,
        tags: Vec::new(),
        // JMAP has no per-object version; the account state is not one.
        version: None,
    })
}

impl MailboxContentReader for JmapApiClientRepository {
    async fn organization(
        &self,
        token: &AccessToken,
        link_id: Uuid,
        id: &ProviderId,
        folders: &[MailFolder],
    ) -> Result<Option<MailboxOrganization>, EmailApiError> {
        let found = self
            .read_only(token)?
            .email_get::<WireEmail>(&[id.as_str().to_owned()], ORGANIZATION_PROPERTIES, None)
            .await?;
        found
            .list
            .into_iter()
            .next()
            .map(|email| normalize(email, link_id, folders).map(Into::into))
            .transpose()
    }

    async fn folders(&self, token: &AccessToken) -> Result<Vec<MailFolder>, EmailApiError> {
        let mailboxes = self.read_only(token)?.mailbox_get().await?;
        folders_from(mailboxes.list)
    }

    async fn message(
        &self,
        token: &AccessToken,
        link_id: Uuid,
        id: &ProviderId,
        folders: &[MailFolder],
    ) -> Result<Option<MailboxMessage>, EmailApiError> {
        let found = self
            .read_only(token)?
            .email_get::<WireEmail>(
                &[id.as_str().to_owned()],
                MESSAGE_PROPERTIES,
                Some(BodyOptions {
                    values: true,
                    max_value_bytes: MAX_BODY_VALUE_BYTES,
                }),
            )
            .await?;
        found
            .list
            .into_iter()
            .next()
            .map(|email| normalize(email, link_id, folders))
            .transpose()
    }

    async fn recent_inbox_messages(
        &self,
        token: &AccessToken,
        link_id: Uuid,
        limit: u16,
        folders: &[MailFolder],
    ) -> Result<Vec<MailboxMessage>, EmailApiError> {
        let Some(inbox) = folders
            .iter()
            .filter(|folder| folder.role == FolderRole::Inbox)
            .map(|folder| folder.id.as_str())
            .min()
        else {
            return Ok(Vec::new());
        };
        let jmap = self.read_only(token)?;
        let page = jmap
            .email_query(&EmailQuery {
                in_mailbox: inbox,
                before: None,
                anchor: None,
                limit: u32::from(limit),
            })
            .await?;
        if page.ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut emails = jmap
            .email_get::<WireEmail>(&page.ids, SUMMARY_PROPERTIES, None)
            .await?
            .list;
        // Email/get promises no order; restore the query's, newest first.
        emails.sort_by_key(|email| page.ids.iter().position(|id| id == &email.id));
        emails
            .into_iter()
            .map(|email| normalize(email, link_id, folders))
            .collect()
    }

    async fn attachment(
        &self,
        token: &AccessToken,
        _message_id: &ProviderId,
        attachment_id: &ProviderId,
    ) -> Result<Vec<u8>, EmailApiError> {
        // An attachment's provider id is its blob id.
        self.read_only(token)?
            .download_blob(attachment_id.as_str())
            .await
    }

    async fn mime(
        &self,
        token: &AccessToken,
        message_id: &ProviderId,
    ) -> Result<Vec<u8>, EmailApiError> {
        let jmap = self.read_only(token)?;
        let found = jmap
            .email_get::<WireEmail>(&[message_id.as_str().to_owned()], &["id", "blobId"], None)
            .await?;
        let blob_id = found
            .list
            .into_iter()
            .next()
            .and_then(|email| email.blob_id)
            .ok_or(EmailApiError::NotFound)?;
        jmap.download_blob(&blob_id).await
    }
}
