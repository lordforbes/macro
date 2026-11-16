use super::*;

#[test]
fn cursors_round_trip_through_tokens() {
    for cursor in [
        GenericCursor::JmapEnumerating {
            state: "c1:7:ab".into(),
            after: "m|199".into(),
        },
        GenericCursor::JmapState {
            state: "c1:9:ab".into(),
        },
        GenericCursor::Imap {
            uidvalidity: 1_700_000_000,
            last_uid: 42,
            highestmodseq: None,
        },
        GenericCursor::Imap {
            uidvalidity: 3,
            last_uid: 0,
            highestmodseq: Some(u64::MAX),
        },
    ] {
        let token = cursor.to_token().unwrap();
        assert_eq!(GenericCursor::from_token(&token).unwrap(), cursor);
    }
}

#[test]
fn foreign_malformed_and_other_version_tokens_are_outdated() {
    for text in [
        "",
        "https://graph.microsoft.com/v1.0/me/mailFolders/delta?$deltatoken=x",
        "s|c1:7:ab",
        r#"{"version":1,"protocol":"pop3","uid":1}"#,
        r#"{"version":2,"protocol":"jmap_state","state":"c1:9:ab"}"#,
        r#"{"protocol":"jmap_state","state":"c1:9:ab"}"#,
    ] {
        assert_eq!(
            GenericCursor::from_token(&StreamToken::new(text.into())),
            Err(EmailApiError::OutdatedCursor),
            "{text}"
        );
    }
}

#[test]
fn an_imap_cursor_is_void_under_another_uidvalidity() {
    let cursor = GenericCursor::Imap {
        uidvalidity: 10,
        last_uid: 77,
        highestmodseq: None,
    };
    assert_eq!(cursor.imap_position(10), Ok(77));
    assert_eq!(cursor.imap_position(11), Err(EmailApiError::OutdatedCursor));
    let jmap = GenericCursor::JmapState { state: "s".into() };
    assert_eq!(jmap.imap_position(10), Err(EmailApiError::OutdatedCursor));
}

#[test]
fn jmap_roles_map_and_unknown_roles_are_ordinary_folders() {
    assert_eq!(folder_role_from_jmap(Some("inbox")), FolderRole::Inbox);
    assert_eq!(folder_role_from_jmap(Some("Sent")), FolderRole::Sent);
    assert_eq!(folder_role_from_jmap(Some("drafts")), FolderRole::Drafts);
    assert_eq!(folder_role_from_jmap(Some("archive")), FolderRole::Archive);
    assert_eq!(folder_role_from_jmap(Some("trash")), FolderRole::Trash);
    assert_eq!(folder_role_from_jmap(Some("junk")), FolderRole::Junk);
    for other in [None, Some("important"), Some("all"), Some("")] {
        assert_eq!(folder_role_from_jmap(other), FolderRole::Other);
    }
}

#[test]
fn imap_special_use_wins_over_the_folder_name() {
    assert_eq!(
        folder_role_from_imap(["\\HasNoChildren", "\\Trash"], "Sent"),
        FolderRole::Trash
    );
    assert_eq!(folder_role_from_imap(["\\Junk"], "Bulk"), FolderRole::Junk);
    assert_eq!(
        folder_role_from_imap(["\\HasChildren"], "Projects"),
        FolderRole::Other
    );
}

#[test]
fn imap_names_are_the_fallback_without_special_use() {
    for (name, role) in [
        ("INBOX", FolderRole::Inbox),
        ("Sent Items", FolderRole::Sent),
        ("Sent Messages", FolderRole::Sent),
        ("Drafts", FolderRole::Drafts),
        ("Archives", FolderRole::Archive),
        ("Deleted Items", FolderRole::Trash),
        ("Spam", FolderRole::Junk),
        ("Junk E-mail", FolderRole::Junk),
        ("Receipts", FolderRole::Other),
        ("Sent to review", FolderRole::Other),
    ] {
        assert_eq!(
            folder_role_from_imap(["\\HasNoChildren"], name),
            role,
            "{name}"
        );
    }
}

#[test]
fn virtual_and_unselectable_imap_folders_are_not_synced() {
    assert!(imap_folder_is_not_synced(["\\All"]));
    assert!(imap_folder_is_not_synced(["\\HasChildren", "\\Noselect"]));
    assert!(imap_folder_is_not_synced(["\\flagged"]));
    assert!(!imap_folder_is_not_synced(["\\HasNoChildren", "\\Sent"]));
    assert!(!imap_folder_is_not_synced([]));
}

#[test]
fn flags_read_the_same_from_both_protocols() {
    let jmap = GenericFlags::from_jmap_keywords(["$seen", "$Flagged", "$answered", "work"]);
    let imap = GenericFlags::from_imap_flags(["\\Seen", "\\FLAGGED", "\\Answered", "work"]);
    assert_eq!(jmap, imap);
    assert!(jmap.seen && jmap.flagged && jmap.answered && !jmap.draft && !jmap.deleted);
    assert!(GenericFlags::from_imap_flags(["\\Deleted"]).deleted);
    assert_eq!(
        GenericFlags::from_jmap_keywords(["\\Seen"]),
        GenericFlags::default()
    );
}

#[test]
fn mailbox_state_combines_flags_with_the_folder_role() {
    let unread = GenericFlags::default();
    let inbox = unread.mailbox_state(FolderRole::Inbox);
    assert!(inbox.in_inbox && !inbox.is_read && !inbox.in_trash && !inbox.in_junk);
    assert!(unread.mailbox_state(FolderRole::Drafts).is_draft);
    assert!(unread.mailbox_state(FolderRole::Trash).in_trash);
    assert!(unread.mailbox_state(FolderRole::Junk).in_junk);
    let seen = GenericFlags {
        seen: true,
        flagged: true,
        ..GenericFlags::default()
    };
    let archived = seen.mailbox_state(FolderRole::Archive);
    assert!(archived.is_read && archived.is_flagged && !archived.in_inbox);
    assert_eq!(archived.attention, Attention::Unknown);
}
