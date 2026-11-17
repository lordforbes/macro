use std::sync::Mutex;

use super::*;

fn flags(values: &[&str]) -> BTreeSet<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn server() -> ServerSnapshot {
    ServerSnapshot {
        mailboxes: BTreeMap::from([(
            "inbox".to_owned(),
            MailboxSnapshot {
                total: 2,
                unread: 1,
                messages: BTreeMap::from([
                    ("7:1".to_owned(), flags(&["\\Seen"])),
                    ("7:2".to_owned(), flags(&["\\Recent"])),
                ]),
            },
        )]),
    }
}

/// A server whose state the "sync" under test may change.
struct FakeServer(Mutex<ServerSnapshot>);

#[derive(Debug, thiserror::Error)]
#[error("unreachable")]
struct Unreachable;

impl SnapshotSource for FakeServer {
    type Err = Unreachable;

    async fn snapshot(&self) -> Result<ServerSnapshot, Unreachable> {
        Ok(self.0.lock().unwrap().clone())
    }
}

#[test]
fn identical_snapshots_have_no_changes() {
    assert!(changes(&server(), &server()).is_empty());
}

#[test]
fn every_kind_of_change_is_reported() {
    let before = server();
    let mut after = server();
    let inbox = after.mailboxes.get_mut("inbox").unwrap();
    inbox.unread = 0;
    inbox
        .messages
        .insert("7:2".to_owned(), flags(&["\\Seen"]));
    inbox.messages.remove("7:1");
    inbox.messages.insert("7:3".to_owned(), flags(&[]));
    after
        .mailboxes
        .insert("sent".to_owned(), MailboxSnapshot::default());

    assert_eq!(
        changes(&before, &after),
        vec![
            SnapshotChange::CountsChanged {
                mailbox: "inbox".to_owned(),
                before: (2, 1),
                after: (2, 0),
            },
            SnapshotChange::MessageRemoved {
                mailbox: "inbox".to_owned(),
                message: "7:1".to_owned(),
            },
            SnapshotChange::FlagsChanged {
                mailbox: "inbox".to_owned(),
                message: "7:2".to_owned(),
                added: flags(&["\\Seen"]),
                removed: flags(&["\\Recent"]),
            },
            SnapshotChange::MessageAdded {
                mailbox: "inbox".to_owned(),
                message: "7:3".to_owned(),
            },
            SnapshotChange::MailboxAdded {
                mailbox: "sent".to_owned(),
            },
        ]
    );
}

#[tokio::test]
async fn a_run_that_only_reads_passes_and_returns_its_output() {
    let server = FakeServer(Mutex::new(server()));

    let output = assert_read_only(&server, async { 42 }).await.unwrap();

    assert_eq!(output, 42);
}

#[tokio::test]
async fn a_run_that_marks_a_message_read_fails() {
    let server = FakeServer(Mutex::new(server()));

    let result = assert_read_only(&server, async {
        // What a plain BODY[] fetch does to an unread IMAP message.
        let mut state = server.0.lock().unwrap();
        let inbox = state.mailboxes.get_mut("inbox").unwrap();
        inbox.unread = 0;
        inbox
            .messages
            .insert("7:2".to_owned(), flags(&["\\Recent", "\\Seen"]));
    })
    .await;

    let Err(ReadOnlyCheckError::Changed(changes)) = result else {
        panic!("expected the change to be caught");
    };
    assert_eq!(changes.len(), 2, "{changes:?}");
}
