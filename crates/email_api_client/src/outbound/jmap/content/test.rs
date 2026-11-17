use serde_json::json;

use super::*;

const LINK: Uuid = Uuid::from_u128(7);

/// Mailflare's layout: one container per address, role folders beneath it.
fn mailflare_mailboxes() -> Vec<WireMailbox> {
    serde_json::from_value(json!([
        {"id": "top", "name": "reader <reader@example.com>", "parentId": null, "role": null, "totalEmails": 14},
        {"id": "inbox", "name": "Inbox", "parentId": "top", "role": "inbox", "totalEmails": 12},
        {"id": "sent", "name": "Sent", "parentId": "top", "role": "sent", "totalEmails": 1},
        {"id": "trash", "name": "Trash", "parentId": "top", "role": "trash", "totalEmails": 1},
        {"id": "projects", "name": "Projects", "parentId": "top", "role": null, "totalEmails": 0},
        {"id": "nested", "name": "2026", "parentId": "projects", "role": null, "totalEmails": 0}
    ]))
    .unwrap()
}

fn email(value: serde_json::Value) -> WireEmail {
    serde_json::from_value(value).unwrap()
}

#[test]
fn account_containers_are_not_folders_but_user_folders_are() {
    let folders = folders_from(mailflare_mailboxes()).unwrap();
    let names: Vec<(&str, FolderRole, Option<&str>)> = folders
        .iter()
        .map(|folder| {
            (
                folder.id.as_str(),
                folder.role,
                folder.parent_id.as_ref().map(ProviderId::as_str),
            )
        })
        .collect();

    assert_eq!(
        names,
        vec![
            ("inbox", FolderRole::Inbox, None),
            ("sent", FolderRole::Sent, None),
            ("trash", FolderRole::Trash, None),
            ("projects", FolderRole::Other, None),
            ("nested", FolderRole::Other, Some("projects")),
        ]
    );
    let projects = folders
        .iter()
        .find(|folder| folder.id.as_str() == "projects")
        .unwrap();
    assert!(projects.has_children);
}

#[test]
fn a_message_reads_flags_folder_role_and_attachments() {
    let folders = folders_from(mailflare_mailboxes()).unwrap();
    let message = normalize(
        email(json!({
            "id": "e1",
            "mailboxIds": {"inbox": true},
            "keywords": {"$seen": true, "$flagged": true},
            "messageId": ["a@sender.example"],
            "from": [{"name": "Alice", "email": "alice@sender.example"}],
            "to": [{"name": null, "email": "reader@example.com"}],
            "subject": "Report",
            "receivedAt": "2026-11-01T09:00:00Z",
            "htmlBody": [{"partId": "1", "type": "text/html"}],
            "bodyValues": {"1": {"value": "<p>Hello</p>"}},
            "attachments": [{"partId": "2", "blobId": "att~2", "name": "small.csv", "type": "text/csv", "size": 14}]
        })),
        LINK,
        &folders,
    )
    .unwrap();

    assert!(message.state.is_read && message.state.is_flagged && message.state.in_inbox);
    assert_eq!(
        message.folder_id.as_ref().map(ProviderId::as_str),
        Some("inbox")
    );
    let content = &message.content.message;
    assert!(!content.is_sent);
    assert_eq!(content.global_id.as_deref(), Some("<a@sender.example>"));
    assert_eq!(content.attachments.len(), 1);
    assert_eq!(content.attachments[0].provider_id.as_deref(), Some("att~2"));
    assert!(
        content
            .body_html_sanitized
            .as_deref()
            .unwrap()
            .contains("Hello")
    );
}

#[test]
fn a_message_in_sent_is_sent_and_one_in_a_container_has_no_folder() {
    let folders = folders_from(mailflare_mailboxes()).unwrap();
    let sent = normalize(
        email(json!({"id": "s", "mailboxIds": {"sent": true}, "keywords": {}})),
        LINK,
        &folders,
    )
    .unwrap();
    let stray = normalize(
        email(json!({"id": "x", "mailboxIds": {"top": true}, "keywords": {}})),
        LINK,
        &folders,
    )
    .unwrap();

    assert!(sent.content.message.is_sent);
    assert!(stray.folder_id.is_none());
}

#[test]
fn a_reply_chain_gets_one_thread_whatever_the_server_says() {
    let folders = folders_from(mailflare_mailboxes()).unwrap();
    // Mailflare named each of these after its own Message-ID because the
    // replies arrived first; the headers still say one conversation.
    let root = normalize(
        email(
            json!({"id": "r", "mailboxIds": {"inbox": true}, "threadId": "root@x",
            "messageId": ["root@x"], "subject": "Plans"}),
        ),
        LINK,
        &folders,
    )
    .unwrap();
    let reply = normalize(
        email(
            json!({"id": "a", "mailboxIds": {"inbox": true}, "threadId": "reply-2@x",
            "messageId": ["reply-2@x"], "inReplyTo": ["reply-1@x"],
            "references": ["root@x", "reply-1@x"], "subject": "Re: Plans"}),
        ),
        LINK,
        &folders,
    )
    .unwrap();

    assert_eq!(
        root.content.message.provider_thread_id,
        reply.content.message.provider_thread_id
    );
}
