use std::{
    collections::{HashMap, HashSet},
    num::NonZeroU16,
    sync::Mutex,
};

use generic_mailbox_credentials::{
    cipher::{DataKeyError, GeneratedDataKey},
    connection::{ImapHost, ImapTls},
    secret::MailboxSecret,
};
use zeroize::Zeroizing;

use super::*;

const LINK: Uuid = Uuid::from_u128(1);
const OTHER_LINK: Uuid = Uuid::from_u128(2);

#[derive(Debug, thiserror::Error)]
#[error("test storage failure")]
struct TestError;

/// Bindings for the generic links in `links`; any other id is not found.
#[derive(Default)]
struct FakeBindings {
    links: HashSet<Uuid>,
    rows: Mutex<HashMap<Uuid, StoredBinding>>,
}

impl GenericMailboxBindings for FakeBindings {
    type Err = TestError;

    async fn store(
        &self,
        link_id: Uuid,
        server: &MailboxServer,
        username: &MailboxUsername,
        encrypted: &EncryptedMailboxSecret,
    ) -> Result<BindingWrite, TestError> {
        if !self.links.contains(&link_id) {
            return Ok(BindingWrite::LinkNotFound);
        }
        self.rows.lock().unwrap().insert(
            link_id,
            StoredBinding {
                server: server.clone(),
                username: username.clone(),
                encrypted: encrypted.clone(),
            },
        );
        Ok(BindingWrite::Stored)
    }

    async fn binding(&self, link_id: Uuid) -> Result<Option<StoredBinding>, TestError> {
        Ok(self.rows.lock().unwrap().get(&link_id).cloned())
    }
}

/// One fixed data key, refused under any context but the one it was made in.
#[derive(Default)]
struct FakeKms {
    contexts: Mutex<Vec<HashMap<String, String>>>,
}

impl DataKeyProvider for FakeKms {
    async fn generate_data_key(
        &self,
        encryption_context: HashMap<String, String>,
    ) -> Result<GeneratedDataKey, DataKeyError> {
        self.contexts.lock().unwrap().push(encryption_context);
        Ok(GeneratedDataKey {
            plaintext: Zeroizing::new(vec![3; 32]),
            encrypted: vec![9],
            key_id: "test-key".to_owned(),
        })
    }

    async fn decrypt_data_key(
        &self,
        _key_id: &str,
        _encrypted_data_key: &[u8],
        encryption_context: HashMap<String, String>,
    ) -> Result<Zeroizing<Vec<u8>>, DataKeyError> {
        if !self.contexts.lock().unwrap().contains(&encryption_context) {
            return Err(DataKeyError);
        }
        Ok(Zeroizing::new(vec![3; 32]))
    }
}

fn service(links: &[Uuid]) -> GenericMailboxService<FakeBindings, FakeKms> {
    GenericMailboxService::new(
        FakeBindings {
            links: links.iter().copied().collect(),
            ..FakeBindings::default()
        },
        FakeKms::default(),
    )
}

fn credentials(secret: &str) -> MailboxCredentials {
    MailboxCredentials {
        server: MailboxServer::Imap {
            host: ImapHost::parse("imap.example.com").unwrap(),
            port: NonZeroU16::new(993).unwrap(),
            tls: ImapTls::Implicit,
        },
        username: MailboxUsername::parse("someone@example.com").unwrap(),
        secret: MailboxSecret::parse(secret.to_owned()).unwrap(),
    }
}

#[tokio::test]
async fn stored_credentials_open_for_their_link() {
    let service = service(&[LINK]);

    service.store(LINK, credentials("app password")).await.unwrap();
    let opened = service.credentials(LINK).await.unwrap();

    assert_eq!(opened.secret.expose(), "app password");
    assert_eq!(opened.server, credentials("x").server);
    assert_eq!(opened.username, credentials("x").username);
}

#[tokio::test]
async fn only_ciphertext_is_stored() {
    let service = service(&[LINK]);

    service.store(LINK, credentials("app password")).await.unwrap();

    let rows = service.bindings.rows.lock().unwrap();
    let stored = &rows[&LINK].encrypted.secret_ciphertext;
    assert!(!stored.windows(8).any(|window| window == b"password"));
}

#[tokio::test]
async fn storing_for_a_link_that_is_not_generic_is_not_found() {
    let service = service(&[LINK]);

    let result = service.store(OTHER_LINK, credentials("x")).await;

    assert!(matches!(result, Err(GenericMailboxError::NotFound)));
}

#[tokio::test]
async fn reading_a_link_without_credentials_is_not_found() {
    let service = service(&[LINK]);

    let result = service.credentials(LINK).await;

    assert!(matches!(result, Err(GenericMailboxError::NotFound)));
}

#[tokio::test]
async fn a_row_moved_to_another_link_does_not_open() {
    let service = service(&[LINK, OTHER_LINK]);
    service.store(LINK, credentials("x")).await.unwrap();
    let moved = service.bindings.rows.lock().unwrap()[&LINK].clone();
    service.bindings.rows.lock().unwrap().insert(OTHER_LINK, moved);

    let result = service.credentials(OTHER_LINK).await;

    assert!(matches!(result, Err(GenericMailboxError::Unavailable(_))));
}
