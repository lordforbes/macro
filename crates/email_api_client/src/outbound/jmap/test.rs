use std::num::NonZeroU16;

use generic_mailbox_credentials::connection::{
    ImapHost, ImapTls, JmapOrigin, MailboxServer, MailboxUsername,
};
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

use super::*;

const KEY: &str = "test-api-key";
const ACCOUNT: &str = "account-1";

fn token_for(server: MailboxServer) -> AccessToken {
    let credentials = MailboxCredentials {
        server,
        username: MailboxUsername::parse("someone@example.com").unwrap(),
        secret: MailboxSecret::parse(KEY.to_owned()).unwrap(),
    };
    AccessToken::new(serde_json::to_string(&credentials).unwrap())
}

fn jmap_token(origin: &str) -> AccessToken {
    token_for(MailboxServer::Jmap {
        endpoint: JmapOrigin::parse(origin).unwrap(),
    })
}

fn session(api_origin: &str, download_origin: &str) -> Value {
    json!({
        "capabilities": {
            "urn:ietf:params:jmap:core": {},
            "urn:ietf:params:jmap:mail": {},
            "urn:example:unknown": {"anything": true}
        },
        "apiUrl": format!("{api_origin}/jmap/api"),
        "downloadUrl": format!("{download_origin}/jmap/download/{{accountId}}/{{blobId}}/{{name}}?type={{type}}"),
        "primaryAccounts": {"urn:ietf:params:jmap:mail": ACCOUNT}
    })
}

/// A server whose discovery redirects once, as Mailflare's does.
async fn server() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/.well-known/jmap"))
        .respond_with(ResponseTemplate::new(301).insert_header("location", "/jmap/session"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/jmap/session"))
        .and(header("authorization", format!("Bearer {KEY}")))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(session(&server.uri(), &server.uri())),
        )
        .mount(&server)
        .await;
    server
}

fn api_reply(responses: Value) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .set_body_json(json!({"methodResponses": responses, "sessionState": "s"}))
}

fn body(request: &Request) -> Value {
    serde_json::from_slice(&request.body).unwrap()
}

#[tokio::test]
async fn a_call_declares_only_core_and_mail_and_names_the_account() {
    let server = server().await;
    Mock::given(method("POST"))
        .and(path("/jmap/api"))
        .respond_with(api_reply(json!([[
            "Email/changes",
            {"oldState": "1", "newState": "2", "hasMoreChanges": false,
             "created": ["e1"], "updated": [], "destroyed": ["e0"]},
            "c0"
        ]])))
        .mount(&server)
        .await;
    let repository = JmapApiClientRepository::new().unwrap();
    let token = jmap_token(&server.uri());

    let changes = repository
        .read_only(&token)
        .unwrap()
        .email_changes("1", 100)
        .await
        .unwrap();

    assert_eq!(changes.created, vec!["e1"]);
    assert_eq!(changes.destroyed, vec!["e0"]);
    let requests = server.received_requests().await.unwrap();
    let call = requests
        .iter()
        .find(|request| request.url.path() == "/jmap/api")
        .unwrap();
    let sent = body(call);
    assert_eq!(
        sent["using"],
        json!(["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"])
    );
    assert_eq!(sent["methodCalls"][0][0], "Email/changes");
    assert_eq!(sent["methodCalls"][0][1]["accountId"], ACCOUNT);
    assert_eq!(
        call.headers.get("authorization").unwrap(),
        &format!("Bearer {KEY}")
    );
}

#[tokio::test]
async fn discovery_refuses_a_redirect_to_another_origin() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/.well-known/jmap"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("location", "https://elsewhere.example/jmap"),
        )
        .mount(&server)
        .await;
    let repository = JmapApiClientRepository::new().unwrap();
    let token = jmap_token(&server.uri());

    let result = repository.read_only(&token).unwrap().core_echo().await;

    assert!(matches!(result, Err(EmailApiError::Permanent { .. })));
}

#[tokio::test]
async fn a_session_pointing_the_api_elsewhere_is_refused() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/.well-known/jmap"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(session("https://elsewhere.example", &server.uri())),
        )
        .mount(&server)
        .await;
    let repository = JmapApiClientRepository::new().unwrap();
    let token = jmap_token(&server.uri());

    let result = repository.read_only(&token).unwrap().core_echo().await;

    assert!(matches!(result, Err(EmailApiError::Permanent { .. })));
    let api_calls = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|request| request.method.as_str() == "POST")
        .count();
    assert_eq!(api_calls, 0);
}

#[tokio::test]
async fn a_session_pointing_downloads_elsewhere_is_refused() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/.well-known/jmap"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(session(&server.uri(), "https://elsewhere.example")),
        )
        .mount(&server)
        .await;
    let repository = JmapApiClientRepository::new().unwrap();
    let token = jmap_token(&server.uri());

    let result = repository.read_only(&token).unwrap().core_echo().await;

    assert!(matches!(result, Err(EmailApiError::Permanent { .. })));
}

#[tokio::test]
async fn method_errors_map_to_provider_neutral_failures() {
    let cases = [
        ("cannotCalculateChanges", "outdated"),
        ("anchorNotFound", "outdated"),
        ("unknownMethod", "unsupported"),
        ("forbidden", "forbidden"),
        ("serverFail", "transient"),
    ];
    for (error_type, expected) in cases {
        let server = server().await;
        Mock::given(method("POST"))
            .and(path("/jmap/api"))
            .respond_with(api_reply(json!([["error", {"type": error_type}, "c0"]])))
            .mount(&server)
            .await;
        let repository = JmapApiClientRepository::new().unwrap();
        let token = jmap_token(&server.uri());

        let error = repository
            .read_only(&token)
            .unwrap()
            .email_changes("1", 10)
            .await
            .unwrap_err();

        let kind = match error {
            EmailApiError::OutdatedCursor => "outdated",
            EmailApiError::Unsupported { .. } => "unsupported",
            EmailApiError::Forbidden => "forbidden",
            EmailApiError::Transient { .. } => "transient",
            other => panic!("{error_type}: unexpected {other:?}"),
        };
        assert_eq!(kind, expected, "{error_type}");
    }
}

#[tokio::test]
async fn a_rejected_key_requires_new_credentials() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/.well-known/jmap"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    let repository = JmapApiClientRepository::new().unwrap();
    let token = jmap_token(&server.uri());

    let result = repository.read_only(&token).unwrap().core_echo().await;

    assert!(matches!(result, Err(EmailApiError::AuthRequired)));
}

#[tokio::test]
async fn blobs_download_from_the_session_template_on_the_same_origin() {
    let server = server().await;
    Mock::given(method("GET"))
        .and(path(format!("/jmap/download/{ACCOUNT}/att~1/download")))
        .and(header("authorization", format!("Bearer {KEY}")))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"attachment".to_vec()))
        .mount(&server)
        .await;
    let repository = JmapApiClientRepository::new().unwrap();
    let token = jmap_token(&server.uri());

    let bytes = repository
        .read_only(&token)
        .unwrap()
        .download_blob("att~1")
        .await
        .unwrap();

    assert_eq!(bytes, b"attachment");
}

#[test]
fn imap_credentials_are_refused() {
    let repository = JmapApiClientRepository::new().unwrap();
    let token = token_for(MailboxServer::Imap {
        host: ImapHost::parse("imap.example.com").unwrap(),
        port: NonZeroU16::new(993).unwrap(),
        tls: ImapTls::Implicit,
    });

    assert!(repository.read_only(&token).is_err());
}

#[test]
fn a_token_that_is_not_credentials_is_refused() {
    let repository = JmapApiClientRepository::new().unwrap();

    assert!(
        repository
            .read_only(&AccessToken::new("ya29.oauth-token"))
            .is_err()
    );
}

/// Live tests against a local JMAP server; ignored by default.
mod live;
