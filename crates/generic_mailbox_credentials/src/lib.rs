#![deny(missing_docs)]
//! Connection settings and the encrypted secret of a generic (JMAP or IMAP)
//! mailbox link.
//!
//! [`connection`] validates where a mailbox lives and who signs in,
//! [`secret`] holds the plaintext password or key, [`cipher`] seals it in a
//! KMS envelope bound to one link, and [`store`] keeps the row.
//!
//! # Threat model
//!
//! A JMAP API key or IMAP password is a long-lived credential with no
//! short-lived form, so it is decrypted on each use and dropped straight
//! after. The authentication service owns the row and the KMS key; other
//! services ask it per use. The row is useless without a KMS call bound to
//! its link, so a row copied to another link does not decrypt.
//!
//! Deleting the row, or its link, does not revoke the secret at the mail
//! server. Any disconnect surface must say so.

/// Encrypting and decrypting a secret with a KMS-backed envelope.
pub mod cipher;
/// Validated connection settings.
pub mod connection;
/// The full sign-in, as the internal routes exchange it.
pub mod credentials;
/// The plaintext secret.
pub mod secret;
/// Reading and writing the binding row.
pub mod store;
