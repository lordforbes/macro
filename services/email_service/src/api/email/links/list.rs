use crate::api::context::{ApiContext, AuthorizationService};
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use macro_authorization::{MacroAuthorizationExtractor, UserOrInternal};
use model::response::ErrorResponse;
use models_email::api;
use utoipa::ToSchema;

use strum_macros::AsRefStr;
use thiserror::Error;

#[derive(Debug, Error, AsRefStr)]
pub enum ListLinksError {
    #[error("Database error")]
    DatabaseError(anyhow::Error),
}

impl IntoResponse for ListLinksError {
    fn into_response(self) -> Response {
        let status_code = match &self {
            ListLinksError::DatabaseError(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };

        (status_code, self.to_string()).into_response()
    }
}

/// The response returned from the list links endpoint
#[derive(Debug, serde::Serialize, serde::Deserialize, ToSchema)]
pub struct ListLinksResponse {
    /// the thread, with messages inside
    pub links: Vec<api::link::Link>,
}

/// List all links belonging to the user.
#[utoipa::path(
    get,
    tag = "Links",
    path = "/email/links",
    operation_id = "list_links",
    responses(
            (status = 200, body=ListLinksResponse),
            (status = 401, body=ErrorResponse),
            (status = 404, body=ErrorResponse),
            (status = 500, body=ErrorResponse),
    )
)]
#[tracing::instrument(skip(ctx, authorization), fields(user_id=authorization.authorization.user.user_context.user_id, fusionauth_user_id=authorization.authorization.user.user_context.fusion_user_id))]
pub async fn list_links_handler(
    State(ctx): State<ApiContext>,
    authorization: MacroAuthorizationExtractor<AuthorizationService, UserOrInternal>,
) -> Result<Response, ListLinksError> {
    let inboxes = ctx
        .inbox_catalog
        .list(&authorization.authorization.user.macro_user_id)
        .await
        .map_err(|error| ListLinksError::DatabaseError(anyhow::Error::new(error)))?;
    let links = inboxes
        .into_iter()
        .map(|entry| {
            let sync_status = match entry.sync_status {
                email::domain::models::EmailSyncStatus::Syncing => api::link::SyncStatus::Syncing,
                email::domain::models::EmailSyncStatus::UpToDate => api::link::SyncStatus::UpToDate,
                email::domain::models::EmailSyncStatus::Error => api::link::SyncStatus::Error,
                email::domain::models::EmailSyncStatus::NeedsReauth => {
                    api::link::SyncStatus::NeedsReauth
                }
                email::domain::models::EmailSyncStatus::Inactive => api::link::SyncStatus::Inactive,
            };
            let email::domain::models::MailboxCapabilities {
                send,
                organize,
                drafts,
                push,
                contacts,
                sender_rules,
                tags,
                calendar,
                server_threads,
            } = entry.capabilities;
            let capabilities = api::link::MailboxCapabilities {
                send,
                organize,
                drafts,
                push,
                contacts,
                sender_rules,
                tags,
                calendar,
                server_threads,
            };
            let inbox = entry.facts;
            let link = inbox.link;
            api::link::Link {
                id: link.id,
                macro_id: link.macro_id,
                fusionauth_user_id: link.fusionauth_user_id,
                email_address: link.email_address,
                photo_url: inbox.photo_url,
                provider: api::link::UserProvider::from(link.provider),
                capabilities,
                is_sync_active: link.is_sync_active,
                sync_status,
                needs_reauth: link.needs_reauth,
                needs_calendar_permission: entry.needs_calendar_permission,
                calendar_disabled: inbox.calendar_disabled,
                has_calendar_data: inbox.has_calendar_data,
                settings: api::settings::Settings::from(inbox.settings),
                is_primary: link.is_primary,
                created_at: link.created_at,
                updated_at: link.updated_at,
            }
        })
        .collect();

    Ok((StatusCode::OK, Json(ListLinksResponse { links })).into_response())
}
