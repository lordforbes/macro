use super::*;

#[test]
fn jmap_origin_keeps_only_the_origin_of_an_https_url() {
    let origin = JmapOrigin::parse(" https://Mail.Example.com/.well-known/jmap ").unwrap();
    assert_eq!(origin.as_str(), "https://mail.example.com");

    let with_port = JmapOrigin::parse("https://mail.example.com:8443").unwrap();
    assert_eq!(with_port.as_str(), "https://mail.example.com:8443");
}

#[test]
fn jmap_origin_allows_plain_http_only_to_this_machine_or_a_local_name() {
    assert_eq!(
        JmapOrigin::parse("http://localhost:8787").unwrap().as_str(),
        "http://localhost:8787"
    );
    assert!(JmapOrigin::parse("http://127.0.0.1:8787").is_ok());
    assert!(JmapOrigin::parse("http://[::1]:8787").is_ok());
    // A container name on the local stack's network.
    assert_eq!(
        JmapOrigin::parse("http://Mailflare:3110").unwrap().as_str(),
        "http://mailflare:3110"
    );

    for refused in [
        "http://mail.example.com",
        "http://mail.example.com.",
        "http://10.0.0.1",
        "http://[fd00::1]",
        "http://167772161",
        "ftp://mail.example.com",
        "https://user:pass@mail.example.com",
        "mail.example.com",
        "",
    ] {
        assert_eq!(
            JmapOrigin::parse(refused),
            Err(InvalidConnection::Endpoint),
            "{refused}"
        );
    }
}

#[test]
fn imap_host_is_a_bare_host_name() {
    assert_eq!(
        ImapHost::parse(" IMAP.Example.com ").unwrap().as_str(),
        "imap.example.com"
    );
    for refused in [
        "imaps://imap.example.com",
        "imap.example.com:993",
        "imap.example.com/inbox",
        "user@imap.example.com",
        "imap example.com",
        "",
    ] {
        assert_eq!(
            ImapHost::parse(refused),
            Err(InvalidConnection::Host),
            "{refused}"
        );
    }
}

#[test]
fn servers_read_from_the_wire_with_the_same_checks() {
    let imap: MailboxServer = serde_json::from_str(
        r#"{"protocol":"imap","host":"Imap.Example.com","port":993,"tls":"starttls"}"#,
    )
    .unwrap();
    assert_eq!(
        imap,
        MailboxServer::Imap {
            host: ImapHost::parse("imap.example.com").unwrap(),
            port: NonZeroU16::new(993).unwrap(),
            tls: ImapTls::Starttls,
        }
    );

    for refused in [
        r#"{"protocol":"imap","host":"imap.example.com","port":0,"tls":"implicit"}"#,
        r#"{"protocol":"imap","host":"imap.example.com","port":143,"tls":"none"}"#,
        r#"{"protocol":"jmap","endpoint":"http://mail.example.com"}"#,
        r#"{"protocol":"pop3","endpoint":"https://mail.example.com"}"#,
    ] {
        assert!(
            serde_json::from_str::<MailboxServer>(refused).is_err(),
            "{refused}"
        );
    }
}

#[test]
fn username_is_trimmed_and_must_be_printable() {
    assert_eq!(
        MailboxUsername::parse(" someone@example.com\n")
            .unwrap()
            .as_str(),
        "someone@example.com"
    );
    assert_eq!(
        MailboxUsername::parse("   "),
        Err(InvalidConnection::Username)
    );
    assert_eq!(
        MailboxUsername::parse("some\u{7}one"),
        Err(InvalidConnection::Username)
    );
    assert_eq!(
        MailboxUsername::parse(&"a".repeat(321)),
        Err(InvalidConnection::Username)
    );
}

#[test]
fn debug_output_never_names_the_mailbox() {
    let server = MailboxServer::Jmap {
        endpoint: JmapOrigin::parse("https://mail.example.com").unwrap(),
    };
    let username = MailboxUsername::parse("someone@example.com").unwrap();
    let printed = format!("{server:?} {username:?}");
    assert!(!printed.contains("example.com"), "{printed}");
    assert!(printed.contains("Jmap"), "{printed}");
}
