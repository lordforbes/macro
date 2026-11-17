/// A feature a mailbox provider may lack.
///
/// Gmail and Outlook support every feature they offer today. The generic
/// providers (JMAP and IMAP) lack several, and the read-only phases of their
/// adapters lack every write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailboxCapability {
    /// Reading mailbox content through the provider. Only missing while a
    /// provider's adapter is not wired into the service.
    Read,
    /// Sending mail.
    Send,
    /// Changing read, flagged and folder state on the server.
    Organize,
    /// Drafts stored on the server.
    Drafts,
    /// Change notifications pushed by the server.
    Push,
    /// The provider's address book.
    Contacts,
    /// Server-side sender rules and blocked senders.
    SenderRules,
    /// Labels, categories or tags beyond folders.
    Tags,
    /// The provider's calendar.
    Calendar,
}
