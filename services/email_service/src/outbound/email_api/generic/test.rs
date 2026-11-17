use std::sync::Mutex;

use macro_db_migrator::MACRO_DB_MIGRATIONS;
use sqlx::PgPool;

use super::*;

/// Records which gateway answered.
#[derive(Default)]
struct Recording(Mutex<Vec<&'static str>>, &'static str);

impl MailboxGateway for Recording {
    async fn folders(&self, _: MailboxKey) -> Result<Vec<MailFolder>, EmailApiError> {
        self.0.lock().unwrap().push(self.1);
        Ok(Vec::new())
    }
    async fn changes(
        &self,
        _: MailboxKey,
        _: &ProviderId,
        _: Option<&StreamToken>,
    ) -> Result<MailboxChangePage, EmailApiError> {
        unimplemented!("not routed in these tests")
    }
    async fn message(
        &self,
        _: MailboxKey,
        _: &ProviderId,
        _: &[MailFolder],
    ) -> Result<Option<MailboxMessage>, EmailApiError> {
        self.0.lock().unwrap().push(self.1);
        Ok(None)
    }
    async fn admitted_requests(&self, _: MailboxKey) -> Option<u64> {
        None
    }
}

async fn link(pool: &PgPool, provider: &str) -> MailboxKey {
    let id = Uuid::now_v7();
    sqlx::query!(
        r#"INSERT INTO email_links (id, macro_id, fusionauth_user_id, email_address, provider, is_sync_active)
        VALUES ($1, 'macro|owner@example.com', 'owner', $2, $3::text::email_user_provider_enum, true)"#,
        id,
        format!("{id}@example.com"),
        provider,
    )
    .execute(pool)
    .await
    .unwrap();
    MailboxKey {
        link_id: id,
        sync_generation: 1,
        grant_generation: 0,
    }
}

fn router(pool: &PgPool) -> RoutedMailboxGateway<Recording, Recording> {
    RoutedMailboxGateway {
        outlook: Recording(Mutex::default(), "outlook"),
        jmap: Recording(Mutex::default(), "jmap"),
        providers: LinkProviders::new(pool.clone()),
    }
}

#[sqlx::test(migrator = "MACRO_DB_MIGRATIONS")]
async fn each_link_reaches_its_own_providers_adapter(pool: PgPool) {
    let outlook = link(&pool, "OUTLOOK").await;
    let jmap = link(&pool, "JMAP").await;
    let gateway = router(&pool);

    gateway.folders(outlook).await.unwrap();
    gateway.folders(jmap).await.unwrap();
    gateway
        .message(jmap, &ProviderId::new("e1").unwrap(), &[])
        .await
        .unwrap();

    assert_eq!(*gateway.outlook.0.lock().unwrap(), vec!["outlook"]);
    assert_eq!(*gateway.jmap.0.lock().unwrap(), vec!["jmap", "jmap"]);
}

#[sqlx::test(migrator = "MACRO_DB_MIGRATIONS")]
async fn imap_and_gmail_links_reach_no_adapter(pool: PgPool) {
    let imap = link(&pool, "IMAP").await;
    let gmail = link(&pool, "GMAIL").await;
    let gateway = router(&pool);

    let imap_result = gateway.folders(imap).await;
    let gmail_result = gateway.folders(gmail).await;

    assert!(matches!(
        imap_result,
        Err(EmailApiError::Unsupported { .. })
    ));
    assert!(matches!(gmail_result, Err(EmailApiError::Permanent { .. })));
    assert!(gateway.outlook.0.lock().unwrap().is_empty());
    assert!(gateway.jmap.0.lock().unwrap().is_empty());
}
