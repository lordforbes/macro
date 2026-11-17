use std::collections::HashMap;

use super::thread_unread::service;
use super::*;
use crate::domain::models::{EmailErr, MessageLabel};

/// The fixture thread's inbox, switched to a read-only generic provider.
async fn make_inbox_jmap(pool: &Pool<Postgres>, link_id: Uuid) -> anyhow::Result<()> {
    sqlx::query!(
        "UPDATE email_links SET provider = 'JMAP' WHERE id = $1",
        link_id
    )
    .execute(pool)
    .await?;
    Ok(())
}

fn label_ids(labels: &HashMap<Uuid, Vec<MessageLabel>>) -> Vec<String> {
    let mut ids: Vec<String> = labels
        .values()
        .flatten()
        .map(|label| label.provider_label_id.clone())
        .collect();
    ids.sort();
    ids
}

#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(
        path = "../../../../fixtures",
        scripts("email_thread_labels", "email_thread_unread")
    )
)]
async fn read_only_inbox_refuses_organization_and_keeps_its_state(
    pool: Pool<Postgres>,
) -> anyhow::Result<()> {
    let thread_id = Uuid::parse_str("33333333-3333-3333-3333-333333333333")?;
    let link_id = Uuid::parse_str("aaaaaaaa-aaaa-aaaa-aaaa-bbbbbbbbbbbb")?;
    // The inbox's owner, as the fixture stored it.
    let owner = MacroUserIdStr::try_from(
        sqlx::query_scalar!("SELECT macro_id FROM email_links WHERE id = $1", link_id)
            .fetch_one(&pool)
            .await?,
    )?;
    make_inbox_jmap(&pool, link_id).await?;
    let service = service(pool);
    let before = service
        .email_repo
        .get_thread_label_messages(thread_id, link_id)
        .await?;
    let message_ids: Vec<_> = before.iter().map(|m| m.db_id).collect();
    let labels_before = label_ids(
        &service
            .email_repo
            .labels_by_message_ids(&message_ids)
            .await?,
    );

    assert!(matches!(
        service
            .mark_thread_unread_impl(owner.clone(), thread_id)
            .await,
        Err(EmailErr::ReadOnlyMailbox)
    ));
    assert!(matches!(
        service
            .set_thread_archived_impl(owner.clone(), thread_id, true)
            .await,
        Err(EmailErr::ReadOnlyMailbox)
    ));
    // Opening the thread is not an error, but it changes nothing either.
    assert_eq!(service.mark_thread_seen_impl(owner, thread_id).await?, 0);

    let after = service
        .email_repo
        .get_thread_label_messages(thread_id, link_id)
        .await?;
    assert_eq!(
        before.iter().map(|m| m.is_read).collect::<Vec<_>>(),
        after.iter().map(|m| m.is_read).collect::<Vec<_>>()
    );
    assert_eq!(
        labels_before,
        label_ids(
            &service
                .email_repo
                .labels_by_message_ids(&message_ids)
                .await?
        )
    );
    Ok(())
}
