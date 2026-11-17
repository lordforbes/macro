//! Internal routes for generic (JMAP and IMAP) mailbox credentials: the email
//! service stores them when it connects a mailbox and reads them for each
//! sign-in.

use crate::api::context::{ApiContext, AuthorizationService};
use crate::domain::generic_mailbox::GenericMailboxError;
use axum::{
    Json,
    extract::{Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use generic_mailbox_credentials::credentials::MailboxCredentials;
use macro_authorization::{InternalOnly, MacroAuthorizationExtractor};
use uuid::Uuid;

/// Store or replace a link's credentials. 204 when stored, 404 when the link
/// is not a generic mailbox, 503 when no KMS key is configured or storage fails.
pub async fn store_handler(
    State(ctx): State<ApiContext>,
    _authorization: MacroAuthorizationExtractor<AuthorizationService, InternalOnly>,
    Path(link_id): Path<Uuid>,
    Json(credentials): Json<MailboxCredentials>,
) -> Response {
    let Some(service) = ctx.generic_mailboxes else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    match service.store(link_id, credentials).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => error_response(&error),
    }
}

/// A link's credentials, secret included, for one sign-in. Never cached.
pub async fn read_handler(
    State(ctx): State<ApiContext>,
    _authorization: MacroAuthorizationExtractor<AuthorizationService, InternalOnly>,
    Path(link_id): Path<Uuid>,
) -> Response {
    let Some(service) = ctx.generic_mailboxes else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    match service.credentials(link_id).await {
        Ok(credentials) => {
            ([(header::CACHE_CONTROL, "no-store")], Json(credentials)).into_response()
        }
        Err(error) => error_response(&error),
    }
}

fn error_response(error: &GenericMailboxError) -> Response {
    match error {
        GenericMailboxError::NotFound => StatusCode::NOT_FOUND.into_response(),
        GenericMailboxError::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}
