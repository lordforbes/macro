use std::{num::NonZeroU16, sync::Mutex};

use generic_mailbox_credentials::{
    connection::{ImapHost, ImapTls, JmapOrigin, MailboxServer, MailboxUsername},
    secret::MailboxSecret,
};

use super::*;

#[derive(Debug, thiserror::Error)]
#[error("test failure")]
struct TestError;

#[derive(Default)]
struct FakeLinks {
    already_connected: bool,
    created: Mutex<Vec<(Uuid, UserProvider)>>,
    deleted: Mutex<Vec<Uuid>>,
}

impl GenericLinkRepository for FakeLinks {
    type Err = TestError;

    async fn create_inactive_link(
        &self,
        id: Uuid,
        _request: &ConnectGenericMailbox,
        provider: UserProvider,
    ) -> Result<LinkCreation, TestError> {
        if self.already_connected {
            return Ok(LinkCreation::AlreadyConnected);
        }
        self.created.lock().unwrap().push((id, provider));
        Ok(LinkCreation::Created)
    }

    async fn delete_link(&self, id: Uuid) -> Result<(), TestError> {
        self.deleted.lock().unwrap().push(id);
        Ok(())
    }

    async fn activate(&self, _id: Uuid) -> Result<bool, TestError> {
        Ok(false)
    }
}

#[derive(Default)]
struct FakeCredentials {
    fail: bool,
    stored: Mutex<Vec<Uuid>>,
}

impl GenericCredentialStore for FakeCredentials {
    type Err = TestError;

    async fn store(
        &self,
        link_id: Uuid,
        _credentials: &MailboxCredentials,
    ) -> Result<(), TestError> {
        if self.fail {
            return Err(TestError);
        }
        self.stored.lock().unwrap().push(link_id);
        Ok(())
    }
}

fn request(server: MailboxServer) -> ConnectGenericMailbox {
    ConnectGenericMailbox {
        owner: MacroUserIdStr::try_from("macro|owner@example.com".to_owned()).unwrap(),
        credential_owner: "fusionauth-owner".to_owned(),
        email_address: EmailStr::try_from("owner@example.com".to_owned()).unwrap(),
        credentials: MailboxCredentials {
            server,
            username: MailboxUsername::parse("owner@example.com").unwrap(),
            secret: MailboxSecret::parse("app password".to_owned()).unwrap(),
        },
    }
}

fn jmap() -> MailboxServer {
    MailboxServer::Jmap {
        endpoint: JmapOrigin::parse("https://mail.example.com").unwrap(),
    }
}

fn imap() -> MailboxServer {
    MailboxServer::Imap {
        host: ImapHost::parse("imap.example.com").unwrap(),
        port: NonZeroU16::new(993).unwrap(),
        tls: ImapTls::Implicit,
    }
}

#[tokio::test]
async fn connecting_creates_a_link_of_the_servers_provider_and_stores_its_credentials() {
    let service = GenericConnectionService::new(FakeLinks::default(), FakeCredentials::default());

    let jmap_id = service.connect(request(jmap())).await.unwrap();
    let imap_id = service.connect(request(imap())).await.unwrap();

    assert_eq!(
        *service.links.created.lock().unwrap(),
        vec![(jmap_id, UserProvider::Jmap), (imap_id, UserProvider::Imap)]
    );
    assert_eq!(
        *service.credentials.stored.lock().unwrap(),
        vec![jmap_id, imap_id]
    );
}

#[tokio::test]
async fn a_mailbox_already_connected_is_refused_without_storing_credentials() {
    let service = GenericConnectionService::new(
        FakeLinks {
            already_connected: true,
            ..FakeLinks::default()
        },
        FakeCredentials::default(),
    );

    let result = service.connect(request(jmap())).await;

    assert!(matches!(
        result,
        Err(GenericConnectionError::AlreadyConnected)
    ));
    assert!(service.credentials.stored.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_failed_credential_store_removes_the_new_link() {
    let service = GenericConnectionService::new(
        FakeLinks::default(),
        FakeCredentials {
            fail: true,
            ..FakeCredentials::default()
        },
    );

    let result = service.connect(request(imap())).await;

    assert!(matches!(
        result,
        Err(GenericConnectionError::Unavailable(_))
    ));
    let created = service.links.created.lock().unwrap()[0].0;
    assert_eq!(*service.links.deleted.lock().unwrap(), vec![created]);
}
