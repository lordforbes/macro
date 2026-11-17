//! The read-only JMAP handle: one method per allowed operation, with typed
//! arguments and responses. There is no way to send a method by name.

use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};

use super::method::{ReadCall, ReadMethod};
use super::transport::invalid_response;
use super::{JmapApiClientRepository, JmapCredentials};
use crate::domain::models::EmailApiError;

/// Read-only operations on one link's JMAP server.
pub struct ReadOnlyJmap<'a> {
    repository: &'a JmapApiClientRepository,
    credentials: JmapCredentials,
}

/// A JMAP Mailbox, with the properties the adapter reads.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireMailbox {
    /// The mailbox id.
    pub id: String,
    /// Display name.
    #[serde(default)]
    pub name: String,
    /// Parent mailbox, if nested.
    #[serde(default)]
    pub parent_id: Option<String>,
    /// RFC 8621 role (`inbox`, `sent`, ...), if any.
    #[serde(default)]
    pub role: Option<String>,
    /// Emails in the mailbox, as the server counts them.
    #[serde(default)]
    pub total_emails: Option<u32>,
    /// Unread emails in the mailbox.
    #[serde(default)]
    pub unread_emails: Option<u32>,
}

/// A `/get` response.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetResponse<T> {
    /// The state of the type when read.
    pub state: String,
    /// The objects found.
    pub list: Vec<T>,
    /// Requested ids that do not exist.
    #[serde(default)]
    pub not_found: Vec<String>,
}

/// A `/changes` response.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangesResponse {
    /// The state these changes reach.
    pub new_state: String,
    /// More changes remain after `new_state`.
    pub has_more_changes: bool,
    /// Ids created since the old state.
    #[serde(default)]
    pub created: Vec<String>,
    /// Ids updated since the old state.
    #[serde(default)]
    pub updated: Vec<String>,
    /// Ids destroyed since the old state.
    #[serde(default)]
    pub destroyed: Vec<String>,
}

/// An `Email/query` response.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryResponse {
    /// Matching ids, in the requested order.
    pub ids: Vec<String>,
    /// Index of the first id in the full result.
    #[serde(default)]
    pub position: u64,
    /// Total matches, when the server calculated it.
    #[serde(default)]
    pub total: Option<u64>,
}

/// A Thread: its email ids, oldest first.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireThread {
    /// The thread id.
    pub id: String,
    /// The thread's emails.
    pub email_ids: Vec<String>,
}

/// An `Email/query` page in one mailbox, newest first.
#[derive(Debug, Clone)]
pub struct EmailQuery<'a> {
    /// The mailbox to list.
    pub in_mailbox: &'a str,
    /// Only emails received before this instant (RFC 3339), for windowing.
    pub before: Option<&'a str>,
    /// Continue after this email id.
    pub anchor: Option<&'a str>,
    /// Page size.
    pub limit: u32,
}

impl<'a> ReadOnlyJmap<'a> {
    pub(super) fn new(
        repository: &'a JmapApiClientRepository,
        credentials: JmapCredentials,
    ) -> Self {
        Self {
            repository,
            credentials,
        }
    }

    /// `Core/echo`: check the key and the server answer.
    pub async fn core_echo(&self) -> Result<(), EmailApiError> {
        self.one(ReadMethod::CoreEcho, json!({"ping": true}))
            .await
            .map(|_| ())
    }

    /// `Mailbox/get` of every mailbox.
    pub async fn mailbox_get(&self) -> Result<GetResponse<WireMailbox>, EmailApiError> {
        let arguments = json!({
            "ids": null,
            "properties": ["id", "name", "parentId", "role", "totalEmails", "unreadEmails"],
        });
        self.typed(ReadMethod::MailboxGet, arguments).await
    }

    /// `Mailbox/changes` since a state.
    pub async fn mailbox_changes(&self, since: &str) -> Result<ChangesResponse, EmailApiError> {
        self.typed(ReadMethod::MailboxChanges, json!({"sinceState": since}))
            .await
    }

    /// `Email/query` of one mailbox, newest first.
    pub async fn email_query(
        &self,
        query: &EmailQuery<'_>,
    ) -> Result<QueryResponse, EmailApiError> {
        let mut filter = json!({"inMailbox": query.in_mailbox});
        if let Some(before) = query.before {
            filter["before"] = json!(before);
        }
        let mut arguments = json!({
            "filter": filter,
            "sort": [{"property": "receivedAt", "isAscending": false}],
            "limit": query.limit,
            "calculateTotal": false,
        });
        if let Some(anchor) = query.anchor {
            // Continue after the anchor; stable when emails vanish mid-listing.
            arguments["anchor"] = json!(anchor);
            arguments["anchorOffset"] = json!(1);
        }
        self.typed(ReadMethod::EmailQuery, arguments).await
    }

    /// `Email/get` of the given ids and properties. With no ids, only the
    /// current Email state is read.
    pub async fn email_get<T: DeserializeOwned>(
        &self,
        ids: &[String],
        properties: &[&str],
        body: Option<BodyOptions>,
    ) -> Result<GetResponse<T>, EmailApiError> {
        let mut arguments = json!({"ids": ids, "properties": properties});
        if let Some(body) = body {
            arguments["bodyProperties"] = json!([
                "partId",
                "blobId",
                "size",
                "name",
                "type",
                "charset",
                "disposition",
                "cid"
            ]);
            arguments["fetchTextBodyValues"] = json!(body.values);
            arguments["fetchHTMLBodyValues"] = json!(body.values);
            arguments["maxBodyValueBytes"] = json!(body.max_value_bytes);
        }
        self.typed(ReadMethod::EmailGet, arguments).await
    }

    /// `Email/changes` since a state, at most `max_changes` ids.
    pub async fn email_changes(
        &self,
        since: &str,
        max_changes: u32,
    ) -> Result<ChangesResponse, EmailApiError> {
        let arguments = json!({"sinceState": since, "maxChanges": max_changes});
        self.typed(ReadMethod::EmailChanges, arguments).await
    }

    /// `Thread/get` of the given ids.
    pub async fn thread_get(
        &self,
        ids: &[String],
    ) -> Result<GetResponse<WireThread>, EmailApiError> {
        self.typed(ReadMethod::ThreadGet, json!({"ids": ids})).await
    }

    /// A blob's bytes.
    pub async fn download_blob(&self, blob_id: &str) -> Result<Vec<u8>, EmailApiError> {
        self.repository.download(&self.credentials, blob_id).await
    }

    async fn typed<T: DeserializeOwned>(
        &self,
        method: ReadMethod,
        arguments: Value,
    ) -> Result<T, EmailApiError> {
        let value = self.one(method, arguments).await?;
        serde_json::from_value(value).map_err(|_| invalid_response(method.name()))
    }

    async fn one(&self, method: ReadMethod, arguments: Value) -> Result<Value, EmailApiError> {
        let mut results = self
            .repository
            .call(&self.credentials, vec![ReadCall::new(method, arguments)])
            .await?;
        results
            .pop()
            .ok_or_else(|| invalid_response("a method call was not answered"))
    }
}

/// How much of a message body `Email/get` returns.
#[derive(Debug, Clone, Copy)]
pub struct BodyOptions {
    /// Fetch decoded text and HTML body values.
    pub values: bool,
    /// The largest body value returned, in bytes.
    pub max_value_bytes: u32,
}
