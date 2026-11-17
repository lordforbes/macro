//! Session discovery, method requests and blob downloads. Private to the
//! adapter: it only sends [`ReadCall`]s, and only to the configured origin.

use std::time::Duration;

use reqwest::{Response, StatusCode, header};
use serde::Deserialize;
use serde_json::{Value, json};
use url::Url;

use super::method::ReadCall;
use super::{JmapApiClientRepository, JmapCredentials};
use crate::domain::models::{EmailApiError, RateLimitOrigin};

/// The parts of the JMAP Session object this adapter uses.
#[derive(Clone)]
pub(super) struct Session {
    pub api_url: Url,
    /// RFC 6570 level 1 template with `{accountId}`, `{blobId}`, `{name}`, `{type}`.
    pub download_url: String,
    pub account_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireSession {
    api_url: String,
    download_url: String,
    #[serde(default)]
    primary_accounts: std::collections::HashMap<String, String>,
}

/// One method response: `[name, arguments, callId]`.
#[derive(Deserialize)]
struct WireInvocation(String, Value, String);

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireResponse {
    method_responses: Vec<WireInvocation>,
}

pub(super) fn invalid_response(what: &'static str) -> EmailApiError {
    EmailApiError::Permanent {
        message: format!("invalid JMAP response: {what}"),
    }
}

fn unreachable_server(_: reqwest::Error) -> EmailApiError {
    EmailApiError::Transient {
        message: "JMAP server did not answer".into(),
    }
}

fn status_error(response: &Response) -> Option<EmailApiError> {
    let status = response.status();
    if status.is_success() {
        return None;
    }
    Some(match status {
        StatusCode::UNAUTHORIZED => EmailApiError::AuthRequired,
        StatusCode::FORBIDDEN => EmailApiError::Forbidden,
        StatusCode::NOT_FOUND => EmailApiError::NotFound,
        StatusCode::TOO_MANY_REQUESTS => EmailApiError::RateLimited {
            retry_after: response
                .headers()
                .get(header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.trim().parse::<u64>().ok())
                .map(Duration::from_secs),
            origin: RateLimitOrigin::Provider { concurrency: false },
        },
        status if status.is_server_error() => EmailApiError::Transient {
            message: format!("JMAP server answered {}", status.as_u16()),
        },
        status => EmailApiError::Permanent {
            message: format!("JMAP server answered {}", status.as_u16()),
        },
    })
}

/// A method-level error (RFC 8620 section 3.6.2) as a provider-neutral failure.
pub(super) fn method_error(arguments: &Value) -> EmailApiError {
    match arguments.get("type").and_then(Value::as_str).unwrap_or("") {
        "cannotCalculateChanges" | "anchorNotFound" => EmailApiError::OutdatedCursor,
        "accountNotFound" | "forbidden" => EmailApiError::Forbidden,
        "notFound" => EmailApiError::NotFound,
        "serverUnavailable" | "serverFail" | "serverPartialFail" => EmailApiError::Transient {
            message: "JMAP server could not process the method".into(),
        },
        "unknownMethod" | "unknownCapability" => EmailApiError::Unsupported {
            capability: crate::domain::models::MailboxCapability::Read,
        },
        other => EmailApiError::Permanent {
            message: format!("JMAP method failed: {}", error_token(other)),
        },
    }
}

/// Error types are short tokens; anything else from the server is not echoed.
fn error_token(value: &str) -> &str {
    let token =
        !value.is_empty() && value.len() <= 40 && value.chars().all(|c| c.is_ascii_alphanumeric());
    if token { value } else { "unknown" }
}

/// Whether `url` is on the credential's origin, with nothing that could carry
/// the key elsewhere.
pub(super) fn same_origin(origin: &Url, url: &Url) -> bool {
    url.scheme() == origin.scheme()
        && url.host_str() == origin.host_str()
        && url.port_or_known_default() == origin.port_or_known_default()
        && url.username().is_empty()
        && url.password().is_none()
}

/// Everything but RFC 3986's unreserved characters, which RFC 6570 simple
/// expansion leaves as they are (Mailflare's blob ids contain `~`).
const RESERVED: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// RFC 6570 simple-string expansion of the download template.
fn expand_download(template: &str, account: &str, blob: &str, name: &str, kind: &str) -> String {
    let encode = |value: &str| percent_encoding::utf8_percent_encode(value, RESERVED).to_string();
    template
        .replace("{accountId}", &encode(account))
        .replace("{blobId}", &encode(blob))
        .replace("{name}", &encode(name))
        .replace("{type}", &encode(kind))
}

impl JmapApiClientRepository {
    /// The session for these credentials, discovered once and cached.
    pub(super) async fn session(
        &self,
        credentials: &JmapCredentials,
    ) -> Result<Session, EmailApiError> {
        if let Some(session) = self.cached_session(credentials) {
            return Ok(session);
        }
        let origin = &credentials.origin;
        let mut url = origin
            .join("/.well-known/jmap")
            .map_err(|_| invalid_response("session URL"))?;
        let mut redirected = false;
        let response = loop {
            let response = self
                .client
                .get(url.clone())
                .bearer_auth(credentials.secret.expose())
                .header(header::ACCEPT, "application/json")
                .send()
                .await
                .map_err(unreachable_server)?;
            // Discovery may redirect (RFC 8620 section 2.2). Follow one
            // redirect, and only within the credential's origin.
            if response.status().is_redirection() && !redirected {
                url = response
                    .headers()
                    .get(header::LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| url.join(value).ok())
                    .filter(|target| same_origin(origin, target))
                    .ok_or_else(|| invalid_response("session redirect leaves the origin"))?;
                redirected = true;
                continue;
            }
            if let Some(error) = status_error(&response) {
                return Err(error);
            }
            break response;
        };
        let wire: WireSession = response
            .json()
            .await
            .map_err(|_| invalid_response("session"))?;
        let api_url = Url::parse(&wire.api_url).map_err(|_| invalid_response("apiUrl"))?;
        let account_id = wire
            .primary_accounts
            .get(super::method::MAIL)
            .cloned()
            .filter(|id| !id.is_empty())
            .ok_or_else(|| invalid_response("no primary mail account"))?;
        // The download template is checked with its variables filled, so a
        // template cannot move the key to another host.
        let probe = expand_download(&wire.download_url, &account_id, "b", "n", "t");
        let probe = Url::parse(&probe).map_err(|_| invalid_response("downloadUrl"))?;
        if !same_origin(origin, &api_url) || !same_origin(origin, &probe) {
            return Err(invalid_response("session URLs leave the origin"));
        }
        let session = Session {
            api_url,
            download_url: wire.download_url,
            account_id,
        };
        self.cache_session(credentials, session.clone());
        Ok(session)
    }

    /// Send one request and return each call's arguments, in call order.
    /// A method-level error fails the whole request.
    pub(super) async fn call(
        &self,
        credentials: &JmapCredentials,
        calls: Vec<ReadCall>,
    ) -> Result<Vec<Value>, EmailApiError> {
        let session = self.session(credentials).await?;
        let mut using = vec![super::method::CORE];
        let expected = calls.len();
        let method_calls: Vec<Value> = calls
            .into_iter()
            .enumerate()
            .map(|(index, call)| {
                let ReadCall {
                    method,
                    mut arguments,
                } = call;
                if !using.contains(&method.capability()) {
                    using.push(method.capability());
                }
                if method.takes_account() {
                    arguments.insert("accountId".into(), json!(session.account_id));
                }
                json!([method.name(), arguments, format!("c{index}")])
            })
            .collect();
        let response = self
            .client
            .post(session.api_url.clone())
            .bearer_auth(credentials.secret.expose())
            .json(&json!({"using": using, "methodCalls": method_calls}))
            .send()
            .await
            .map_err(unreachable_server)?;
        if let Some(error) = status_error(&response) {
            // The key may have been revoked or moved to another account.
            if matches!(
                error,
                EmailApiError::AuthRequired | EmailApiError::Forbidden
            ) {
                self.forget_session(credentials);
            }
            return Err(error);
        }
        let wire: WireResponse = response
            .json()
            .await
            .map_err(|_| invalid_response("methodResponses"))?;

        let mut results: Vec<Option<Value>> = vec![None; expected];
        for WireInvocation(name, arguments, call_id) in wire.method_responses {
            let index = call_id
                .strip_prefix('c')
                .and_then(|id| id.parse::<usize>().ok())
                .filter(|index| *index < expected)
                .ok_or_else(|| invalid_response("unknown method call id"))?;
            if name == "error" {
                return Err(method_error(&arguments));
            }
            // A call may answer more than once; the first response is its own.
            if let Some(slot) = results.get_mut(index)
                && slot.is_none()
            {
                *slot = Some(arguments);
            }
        }
        results
            .into_iter()
            .map(|result| result.ok_or_else(|| invalid_response("a method call was not answered")))
            .collect()
    }

    /// A blob's bytes, through the session's download template.
    pub(super) async fn download(
        &self,
        credentials: &JmapCredentials,
        blob_id: &str,
    ) -> Result<Vec<u8>, EmailApiError> {
        let session = self.session(credentials).await?;
        let url = expand_download(
            &session.download_url,
            &session.account_id,
            blob_id,
            "download",
            "application/octet-stream",
        );
        let url = Url::parse(&url).map_err(|_| invalid_response("downloadUrl"))?;
        if !same_origin(&credentials.origin, &url) {
            return Err(invalid_response("download URL leaves the origin"));
        }
        let response = self
            .client
            .get(url)
            .bearer_auth(credentials.secret.expose())
            .send()
            .await
            .map_err(unreachable_server)?;
        if let Some(error) = status_error(&response) {
            return Err(error);
        }
        response
            .bytes()
            .await
            .map(|bytes| bytes.to_vec())
            .map_err(unreachable_server)
    }
}
