//! Email API infrastructure adapters.

mod generic;
mod gmail_gate;
mod microsoft;
mod token_source;

pub use email_api_client::outbound::microsoft_gate::RedisMicrosoftRequestGate;
pub use generic::{GenericCredentialsTokens, LinkProviders, RoutedMailboxGateway};
pub use gmail_gate::{RateBudget, RedisGmailRequestGate};
pub use microsoft::{MicrosoftCredentialsClient, ProviderMailboxGateway};
pub use token_source::{EmailServiceTokenSource, StaticTokenSource};

use email_api_client::GmailApiClientRepository;
use email_api_client::domain::service::EmailApiClientServiceImpl;

/// The JMAP adapter composed with generic mailbox credentials. JMAP servers
/// apply their own limits, so no request gate is attached.
pub type JmapApi = email_api_client::domain::service::mailbox::MailboxApiService<
    email_api_client::JmapApiClientRepository,
    GenericCredentialsTokens,
    email_api_client::domain::ports::AlwaysAllowRateLimiter,
>;

/// Gmail API service composed with email-service infrastructure adapters.
pub type GmailApi = EmailApiClientServiceImpl<
    GmailApiClientRepository,
    EmailServiceTokenSource,
    RedisGmailRequestGate,
>;
