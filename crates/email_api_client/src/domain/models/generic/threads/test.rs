use super::*;

fn headers<'a>(
    message_id: Option<&'a str>,
    in_reply_to: Option<&'a str>,
    references: Option<&'a str>,
) -> ThreadHeaders<'a> {
    ThreadHeaders {
        message_id,
        in_reply_to,
        references,
        subject: Some("Plans"),
    }
}

/// A root message, its reply, and a reply to the reply.
fn conversation() -> [(&'static str, ThreadHeaders<'static>); 3] {
    [
        ("root", headers(Some("<a@example.com>"), None, None)),
        (
            "reply",
            headers(
                Some("<b@example.com>"),
                Some("<a@example.com>"),
                Some("<a@example.com>"),
            ),
        ),
        (
            "reply-to-reply",
            headers(
                Some("<c@example.com>"),
                Some("<b@example.com>"),
                Some("<a@example.com> <b@example.com>"),
            ),
        ),
    ]
}

#[test]
fn replies_share_their_parents_thread() {
    let mut threads = SyntheticThreads::new();
    let ids: Vec<_> = conversation()
        .into_iter()
        .map(|(key, headers)| threads.insert(key, headers).thread_id)
        .collect();

    assert_eq!(ids[0], ids[1]);
    assert_eq!(ids[1], ids[2]);
    assert!(ids[0].as_str().starts_with("syn1-"));
}

#[test]
fn the_thread_id_does_not_depend_on_arrival_order() {
    let messages = conversation();
    let orders = [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ];
    let mut seen = Vec::new();
    for order in orders {
        let mut threads = SyntheticThreads::new();
        for index in order {
            let (key, headers) = messages[index];
            threads.insert(key, headers);
        }
        seen.push(threads.thread_id("root").unwrap());
        assert_eq!(threads.thread_id("root"), threads.thread_id("reply-to-reply"));
    }
    assert!(seen.windows(2).all(|pair| pair[0] == pair[1]), "{seen:?}");
}

#[test]
fn a_message_linking_two_threads_merges_them_and_reports_the_merge() {
    let mut threads = SyntheticThreads::new();
    let first = threads.insert("x", headers(Some("<x@example.com>"), None, None));
    let second = threads.insert("y", headers(Some("<y@example.com>"), None, None));
    assert_ne!(first.thread_id, second.thread_id);

    let joining = threads.insert(
        "z",
        headers(
            Some("<z@example.com>"),
            Some("<y@example.com>"),
            Some("<x@example.com> <y@example.com>"),
        ),
    );

    // The thread named by the smallest id survives; the other is merged into it.
    assert_eq!(joining.thread_id, first.thread_id);
    assert_eq!(joining.merged, vec![second.thread_id]);
    assert_eq!(threads.thread_id("y"), Some(first.thread_id));
}

#[test]
fn replies_to_a_message_not_seen_still_share_a_thread() {
    let mut threads = SyntheticThreads::new();
    let one = threads.insert(
        "one",
        headers(Some("<r1@example.com>"), Some("<missing@example.com>"), None),
    );
    let two = threads.insert(
        "two",
        headers(Some("<r2@example.com>"), Some("<missing@example.com>"), None),
    );

    assert_eq!(one.thread_id, two.thread_id);
    assert!(two.merged.is_empty());
}

#[test]
fn a_message_without_a_message_id_joins_through_what_it_names() {
    let mut threads = SyntheticThreads::new();
    let root = threads.insert("root", headers(Some("<a@example.com>"), None, None));

    let reply = threads.insert("reply", headers(None, Some("<a@example.com>"), None));

    assert_eq!(reply.thread_id, root.thread_id);
}

#[test]
fn messages_sharing_a_message_id_share_a_thread() {
    let mut threads = SyntheticThreads::new();
    let first = threads.insert("copy-1", headers(Some("<same@example.com>"), None, None));
    let second = threads.insert("copy-2", headers(Some("<same@example.com>"), None, None));

    assert_eq!(first.thread_id, second.thread_id);
}

#[test]
fn a_long_references_chain_still_reaches_its_root() {
    let references: String = (0..150)
        .map(|n| format!("<m{n}@example.com>"))
        .collect::<Vec<_>>()
        .join(" ");
    let mut threads = SyntheticThreads::new();
    let root = threads.insert("root", headers(Some("<m0@example.com>"), None, None));

    let deep = threads.insert(
        "deep",
        headers(Some("<deep@example.com>"), None, Some(&references)),
    );

    // A reply never renames the thread it joins.
    assert_eq!(deep.thread_id, root.thread_id);
    assert!(deep.merged.is_empty());
}

#[test]
fn a_malformed_header_is_ignored() {
    let mut threads = SyntheticThreads::new();
    let root = threads.insert("root", headers(Some("<a@example.com>"), None, None));

    let odd = threads.insert(
        "odd",
        headers(Some("not-bracketed@example.com"), Some("<a@example.com"), None),
    );

    assert_ne!(odd.thread_id, root.thread_id);
}

#[test]
fn only_headerless_replies_fall_back_to_the_subject() {
    let mut threads = SyntheticThreads::new();
    let bare = |subject| ThreadHeaders {
        subject: Some(subject),
        ..ThreadHeaders::default()
    };

    let first = threads.insert("first", bare("Re: Quarterly numbers"));
    let second = threads.insert("second", bare("RE[2]: Fwd: quarterly numbers "));
    let original = threads.insert("original", bare("Quarterly numbers"));
    let other_original = threads.insert("other", bare("Quarterly numbers"));

    assert_eq!(first.thread_id, second.thread_id);
    assert_ne!(original.thread_id, first.thread_id);
    assert_ne!(original.thread_id, other_original.thread_id);
}

#[test]
fn indexing_a_message_again_changes_nothing() {
    let mut threads = SyntheticThreads::new();
    let first = threads.insert("root", headers(Some("<a@example.com>"), None, None));

    let again = threads.insert("root", headers(Some("<other@example.com>"), None, None));

    assert_eq!(again.thread_id, first.thread_id);
    assert!(again.merged.is_empty());
}

#[test]
fn learning_the_true_root_renames_the_thread_once() {
    let mut threads = SyntheticThreads::new();
    // A reply to a reply arrives first and names only its direct parent.
    let early = threads.insert(
        "reply-to-reply",
        headers(Some("<c@example.com>"), Some("<b@example.com>"), None),
    );

    // The middle message shows that b itself replies to a.
    let middle = threads.insert(
        "reply",
        headers(Some("<b@example.com>"), Some("<a@example.com>"), None),
    );
    let root = threads.insert("root", headers(Some("<a@example.com>"), None, None));

    assert_eq!(middle.merged, vec![early.thread_id]);
    assert_eq!(root.thread_id, middle.thread_id);
    assert!(root.merged.is_empty());
    assert_eq!(threads.thread_id("reply-to-reply"), Some(root.thread_id));
}
