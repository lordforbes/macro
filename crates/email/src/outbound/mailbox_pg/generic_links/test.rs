use super::*;
use generic_mailbox_credentials::{
    connection::{JmapOrigin, MailboxServer, MailboxUsername},
    credentials::MailboxCredentials,
    secret::MailboxSecret,
};
use macro_db_migrator::MACRO_DB_MIGRATIONS;
use macro_user_id::{email::EmailStr, user_id::MacroUserIdStr};
use sqlx::PgPool;

fn request(address: &str) -> ConnectGenericMailbox {
    ConnectGenericMailbox {
        owner: MacroUserIdStr::try_from("macro|owner@example.com".to_owned()).unwrap(),
        credential_owner: "fusionauth-owner".to_owned(),
        email_address: EmailStr::try_from(address.to_owned()).unwrap(),
        credentials: MailboxCredentials {
            server: MailboxServer::Jmap {
                endpoint: JmapOrigin::parse("https://mail.example.com").unwrap(),
            },
            username: MailboxUsername::parse(address).unwrap(),
            secret: MailboxSecret::parse("key".to_owned()).unwrap(),
        },
    }
}

#[sqlx::test(migrator = "MACRO_DB_MIGRATIONS")]
async fn a_generic_link_is_created_inactive_and_only_once(pool: PgPool) {
    let repo = PgMailboxSync::new(pool.clone());
    let id = Uuid::now_v7();

    let first = repo
        .create_inactive_link(id, &request("Box@Example.com"), UserProvider::Jmap)
        .await
        .unwrap();
    let again = repo
        .create_inactive_link(Uuid::now_v7(), &request("box@example.com"), UserProvider::Jmap)
        .await
        .unwrap();

    assert_eq!(first, LinkCreation::Created);
    assert_eq!(again, LinkCreation::AlreadyConnected);
    let row = sqlx::query!(
        r#"SELECT provider::text AS "provider!", is_sync_active, email_address FROM email_links WHERE id = $1"#,
        id
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.provider, "JMAP");
    assert!(!row.is_sync_active);
    assert_eq!(row.email_address, "box@example.com");
}

#[sqlx::test(migrator = "MACRO_DB_MIGRATIONS")]
async fn cleanup_removes_only_an_inactive_generic_link(pool: PgPool) {
    let repo = PgMailboxSync::new(pool.clone());
    let generic = Uuid::now_v7();
    repo.create_inactive_link(generic, &request("box@example.com"), UserProvider::Imap)
        .await
        .unwrap();
    let gmail = Uuid::now_v7();
    sqlx::query!(
        r#"INSERT INTO email_links (id, macro_id, fusionauth_user_id, email_address, provider, is_sync_active)
        VALUES ($1, 'macro|owner@example.com', 'fusionauth-owner', 'gmail@example.com', 'GMAIL', false)"#,
        gmail
    )
    .execute(&pool)
    .await
    .unwrap();

    repo.delete_link(generic).await.unwrap();
    repo.delete_link(gmail).await.unwrap();

    let remaining = sqlx::query_scalar!(
        r#"SELECT id AS "id!" FROM email_links WHERE id = ANY($1)"#,
        &[generic, gmail][..]
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(remaining, vec![gmail]);
}
