use super::*;
use macro_db_migrator::MACRO_DB_MIGRATIONS;
use sqlx::PgPool;

fn encrypted(seed: u8) -> EncryptedMailboxSecret {
    EncryptedMailboxSecret {
        secret_ciphertext: vec![seed; 32],
        encrypted_data_key: vec![seed, 1, 2],
        nonce: vec![seed; 12],
        encryption_version: 1,
        kms_key_id: format!("arn:aws:kms:us-east-1:123456789012:key/{seed}"),
    }
}

async fn insert_link(pool: &PgPool, provider: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query!(
        r#"
        INSERT INTO email_links (id, macro_id, fusionauth_user_id, email_address, provider, is_sync_active)
        VALUES ($1, 'macro|owner@example.com', 'owner', $2, $3::text::email_user_provider_enum, false)
        "#,
        id,
        format!("{id}@example.com"),
        provider,
    )
    .execute(pool)
    .await
    .unwrap();
    id
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

fn username() -> MailboxUsername {
    MailboxUsername::parse("someone@example.com").unwrap()
}

#[sqlx::test(migrator = "MACRO_DB_MIGRATIONS")]
async fn a_stored_binding_reads_back(pool: PgPool) {
    let jmap_link = insert_link(&pool, "JMAP").await;
    let imap_link = insert_link(&pool, "IMAP").await;

    upsert_binding(&pool, jmap_link, &jmap(), &username(), &encrypted(1))
        .await
        .unwrap();
    upsert_binding(&pool, imap_link, &imap(), &username(), &encrypted(2))
        .await
        .unwrap();

    assert_eq!(
        get_binding(&pool, jmap_link).await.unwrap(),
        Some(StoredBinding {
            server: jmap(),
            username: username(),
            encrypted: encrypted(1),
        })
    );
    assert_eq!(
        get_binding(&pool, imap_link).await.unwrap().unwrap().server,
        imap()
    );
}

#[sqlx::test(migrator = "MACRO_DB_MIGRATIONS")]
async fn storing_again_replaces_the_secret(pool: PgPool) {
    let link = insert_link(&pool, "JMAP").await;
    upsert_binding(&pool, link, &jmap(), &username(), &encrypted(1))
        .await
        .unwrap();

    upsert_binding(&pool, link, &jmap(), &username(), &encrypted(9))
        .await
        .unwrap();

    let stored = get_binding(&pool, link).await.unwrap().unwrap();
    assert_eq!(stored.encrypted, encrypted(9));
}

#[sqlx::test(migrator = "MACRO_DB_MIGRATIONS")]
async fn a_secret_cannot_attach_to_another_providers_link(pool: PgPool) {
    let gmail = insert_link(&pool, "GMAIL").await;
    let imap_link = insert_link(&pool, "IMAP").await;

    let on_gmail = upsert_binding(&pool, gmail, &jmap(), &username(), &encrypted(1)).await;
    let jmap_on_imap =
        upsert_binding(&pool, imap_link, &jmap(), &username(), &encrypted(1)).await;
    let missing =
        upsert_binding(&pool, Uuid::now_v7(), &jmap(), &username(), &encrypted(1)).await;

    assert!(matches!(on_gmail, Err(BindingStoreError::LinkNotFound)));
    assert!(matches!(jmap_on_imap, Err(BindingStoreError::LinkNotFound)));
    assert!(matches!(missing, Err(BindingStoreError::LinkNotFound)));
    assert_eq!(get_binding(&pool, gmail).await.unwrap(), None);
}

#[sqlx::test(migrator = "MACRO_DB_MIGRATIONS")]
async fn deleting_the_link_deletes_its_binding(pool: PgPool) {
    let link = insert_link(&pool, "JMAP").await;
    upsert_binding(&pool, link, &jmap(), &username(), &encrypted(1))
        .await
        .unwrap();

    sqlx::query!("DELETE FROM email_links WHERE id = $1", link)
        .execute(&pool)
        .await
        .unwrap();

    assert_eq!(get_binding(&pool, link).await.unwrap(), None);
}
