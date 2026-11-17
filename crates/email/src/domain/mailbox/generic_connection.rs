//! Connecting a generic (JMAP or IMAP) mailbox: an `email_links` row for the
//! provider, and its credentials in the authentication service.
//!
//! The link is created inactive. Nothing syncs a generic mailbox until its
//! adapter exists, and an inactive link is invisible to every sync worker,
//! including Gmail paths that select links without a provider filter.

use generic_mailbox_credentials::{connection::MailboxProtocol, credentials::MailboxCredentials};
use macro_user_id::{email::EmailStr, user_id::MacroUserIdStr};
use models_email::service::link::UserProvider;
use uuid::Uuid;

#[cfg(test)]
mod test;

/// A request to connect a generic mailbox.
#[derive(Debug)]
pub struct ConnectGenericMailbox {
    /// The Macro user the inbox belongs to.
    pub owner: MacroUserIdStr<'static>,
    /// The owner's FusionAuth user id, the link's credential owner.
    pub credential_owner: String,
    /// The mailbox's address, as Macro shows it.
    pub email_address: EmailStr<'static>,
    /// Where the mailbox lives and how to sign in.
    pub credentials: MailboxCredentials,
}

/// Whether a link row was created.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkCreation {
    /// A new inactive link with the requested id.
    Created,
    /// The owner already has a link for this address and provider.
    AlreadyConnected,
}

/// Link rows for generic mailboxes.
pub trait GenericLinkRepository: Send + Sync + 'static {
    /// The storage failure.
    type Err: std::error::Error + Send + Sync + 'static;

    /// Insert an inactive link, unless the owner already has one for this
    /// address and provider.
    fn create_inactive_link(
        &self,
        id: Uuid,
        request: &ConnectGenericMailbox,
        provider: UserProvider,
    ) -> impl Future<Output = Result<LinkCreation, Self::Err>> + Send;

    /// Remove a link this service created whose credentials could not be stored.
    fn delete_link(&self, id: Uuid) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// Mark an inactive generic link active and queue its first folder
    /// discovery. False when there is no such link to activate.
    fn activate(&self, id: Uuid) -> impl Future<Output = Result<bool, Self::Err>> + Send;
}

/// Where credentials are kept: the authentication service.
pub trait GenericCredentialStore: Send + Sync + 'static {
    /// The failure to store.
    type Err: std::error::Error + Send + Sync + 'static;

    /// Seal and store a link's credentials.
    fn store(
        &self,
        link_id: Uuid,
        credentials: &MailboxCredentials,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;
}

/// Why a mailbox could not be connected or activated.
#[derive(Debug, thiserror::Error)]
pub enum GenericConnectionError {
    /// No inactive generic link with this id exists.
    #[error("no generic mailbox to activate")]
    NotFound,
    /// The owner already connected this mailbox.
    #[error("this mailbox is already connected")]
    AlreadyConnected,
    /// Storage or the authentication service failed; nothing was left behind.
    #[error("the mailbox could not be connected right now")]
    Unavailable(rootcause::Report),
}

/// Connects generic mailboxes.
pub struct GenericConnectionService<R, C> {
    links: R,
    credentials: C,
}

impl<R: GenericLinkRepository, C: GenericCredentialStore> GenericConnectionService<R, C> {
    /// Create links in `links` and store credentials in `credentials`.
    pub fn new(links: R, credentials: C) -> Self {
        Self { links, credentials }
    }

    /// Connect a mailbox and return its new link id.
    #[tracing::instrument(
        name = "generic_mailbox.connect",
        skip_all,
        fields(protocol = %request.credentials.server.protocol(), link_id = tracing::field::Empty),
        err
    )]
    pub async fn connect(
        &self,
        request: ConnectGenericMailbox,
    ) -> Result<Uuid, GenericConnectionError> {
        let provider = match request.credentials.server.protocol() {
            MailboxProtocol::Jmap => UserProvider::Jmap,
            MailboxProtocol::Imap => UserProvider::Imap,
        };
        let id = macro_uuid::generate_uuid_v7();
        tracing::Span::current().record("link_id", tracing::field::display(id));

        let created = self
            .links
            .create_inactive_link(id, &request, provider)
            .await
            .map_err(unavailable)?;
        if created == LinkCreation::AlreadyConnected {
            return Err(GenericConnectionError::AlreadyConnected);
        }

        // A link without credentials can never sync, so a failed store removes
        // the link it was for. If that removal also fails, the inactive row is
        // inert and a retry with the same address reports it as connected.
        if let Err(error) = self.credentials.store(id, &request.credentials).await {
            if let Err(cleanup) = self.links.delete_link(id).await {
                tracing::error!(error = ?cleanup, "could not remove a link without credentials");
            }
            return Err(unavailable(error));
        }
        Ok(id)
    }
}

impl<R: GenericLinkRepository, C> GenericConnectionService<R, C> {
    /// Start syncing a connected mailbox: the link becomes active and the
    /// shared mailbox engine discovers its folders.
    #[tracing::instrument(name = "generic_mailbox.activate", skip(self), err)]
    pub async fn activate(&self, link_id: Uuid) -> Result<(), GenericConnectionError> {
        match self.links.activate(link_id).await.map_err(unavailable)? {
            true => Ok(()),
            false => Err(GenericConnectionError::NotFound),
        }
    }
}

fn unavailable<E: std::error::Error + Send + Sync + 'static>(error: E) -> GenericConnectionError {
    GenericConnectionError::Unavailable(rootcause::Report::new(error).into_dynamic())
}
