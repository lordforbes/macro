//! # Connect a generic mailbox
//!
//! Operator tool that connects a JMAP or IMAP mailbox to a Macro user until
//! the add-account form exists. It creates an inactive `email_links` row and
//! stores the credentials, sealed, in the authentication service. Nothing
//! syncs the mailbox yet: the adapters come in later phases.
//!
//! The password or API key is read from standard input (one line), never
//! from an argument, so it stays out of shell history and process listings.
//!
//! Run it only once every email, authentication and calendar binary is on a
//! release that knows the `JMAP` and `IMAP` providers. Older binaries decode
//! `email_links.provider` strictly and fail on a generic link's row, so do
//! not roll back past that release while a generic link exists; delete the
//! generic links first.
//!
//! ```text
//! printf '%s\n' "$KEY" | connect_generic_mailbox \
//!     --owner 'macro|someone@example.com' --credential-owner <fusionauth id> \
//!     --email someone@example.com --username someone@example.com \
//!     jmap --endpoint https://mail.example.com
//! ```
//!
//! ## Required environment variables
//! - `DATABASE_URL`: MacroDB.
//! - `AUTHENTICATION_SERVICE_SECRET_KEY`: the internal key for the
//!   authentication service, whose address comes from `macro_service_urls`.

use std::{io::BufRead as _, num::NonZeroU16};

use authentication_service_client::AuthServiceClient;
use clap::{Parser, Subcommand};
use email::{
    domain::mailbox::generic_connection::{
        ConnectGenericMailbox, GenericConnectionError, GenericConnectionService,
    },
    outbound::mailbox_pg::PgMailboxSync,
};
use email_service::outbound::generic_credentials::AuthGenericCredentials;
use generic_mailbox_credentials::{
    connection::{ImapHost, ImapTls, JmapOrigin, MailboxServer, MailboxUsername},
    credentials::MailboxCredentials,
    secret::MailboxSecret,
};
use macro_entrypoint::MacroEntrypoint;
use macro_env_var::env_vars;
use macro_service_urls::AuthServiceUrl;
use macro_user_id::{email::EmailStr, user_id::MacroUserIdStr};
use rootcause::prelude::{Report, ResultExt as _};
use sqlx::postgres::PgPoolOptions;

env_vars! {
    struct DatabaseUrl;
    struct AuthenticationServiceSecretKey;
}

#[derive(Parser)]
#[command(about = "Connect a JMAP or IMAP mailbox to a Macro user (secret on stdin)")]
struct Args {
    /// The Macro user id the inbox belongs to (`macro|<email>`).
    #[arg(long)]
    owner: String,
    /// The owner's FusionAuth user id.
    #[arg(long)]
    credential_owner: String,
    /// The mailbox's address, as Macro shows it.
    #[arg(long)]
    email: String,
    /// The name the mailbox signs in with.
    #[arg(long)]
    username: String,
    #[command(subcommand)]
    server: Server,
}

#[derive(Subcommand)]
enum Server {
    /// A JMAP server.
    Jmap {
        /// The server origin, for example `https://mail.example.com`.
        #[arg(long)]
        endpoint: String,
    },
    /// An IMAP server.
    Imap {
        /// The server host name.
        #[arg(long)]
        host: String,
        /// The server port.
        #[arg(long, default_value_t = NonZeroU16::new(993).unwrap_or(NonZeroU16::MIN))]
        port: NonZeroU16,
        /// `implicit` (TLS from the start) or `starttls`.
        #[arg(long, default_value = "implicit")]
        tls: String,
    },
}

#[tokio::main]
async fn main() -> Result<(), Report> {
    let entrypoint = MacroEntrypoint::default().init();
    let result = run().await;
    entrypoint.shutdown();
    result
}

async fn run() -> Result<(), Report> {
    let args = Args::parse();
    let server = match args.server {
        Server::Jmap { endpoint } => MailboxServer::Jmap {
            endpoint: JmapOrigin::parse(&endpoint).context("invalid --endpoint")?,
        },
        Server::Imap { host, port, tls } => MailboxServer::Imap {
            host: ImapHost::parse(&host).context("invalid --host")?,
            port,
            tls: tls
                .parse::<ImapTls>()
                .context("--tls must be implicit or starttls")?,
        },
    };

    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .context("could not read the secret from standard input")?;
    // Only the line ending is removed: a password may end in a space.
    let secret = line.strip_suffix('\n').unwrap_or(&line);
    let secret = secret.strip_suffix('\r').unwrap_or(secret).to_owned();
    zeroize::Zeroize::zeroize(&mut line);

    let request = ConnectGenericMailbox {
        owner: MacroUserIdStr::try_from(args.owner).context("invalid --owner")?,
        credential_owner: args.credential_owner,
        email_address: EmailStr::try_from(args.email).context("invalid --email")?,
        credentials: MailboxCredentials {
            server,
            username: MailboxUsername::parse(&args.username).context("invalid --username")?,
            secret: MailboxSecret::parse(secret).context("the secret on stdin is not usable")?,
        },
    };

    let db = PgPoolOptions::new()
        .max_connections(1)
        .connect(DatabaseUrl::new()?.as_ref())
        .await
        .context("could not connect to the database")?;
    let auth = AuthServiceClient::new(
        AuthenticationServiceSecretKey::new()?.to_string(),
        AuthServiceUrl::new()?.to_string(),
    );
    let service =
        GenericConnectionService::new(PgMailboxSync::new(db), AuthGenericCredentials(auth));

    match service.connect(request).await {
        Ok(link_id) => {
            println!("connected: link {link_id} (inactive until its adapter is enabled)");
            Ok(())
        }
        Err(GenericConnectionError::AlreadyConnected) => Err(rootcause::report!(
            "this owner already has this mailbox connected"
        )),
        Err(error) => Err(Report::new(error).into_dynamic()),
    }
}
