# generic_mailbox_credentials

## What it is

Part of email's generic (JMAP and IMAP) mailboxes: validated connection
settings, the plaintext secret type, the KMS envelope cipher and the
`email_generic_mailbox_bindings` row. See `src/lib.rs` for the threat model.

## Start here

- `src/cipher.rs`: the envelope and why it shares the Microsoft token key.
- `src/store.rs`: the row, written only for a link whose provider matches.

## Scope

- Owns: the binding row and how its secret is sealed.
- Not here: creating the `email_links` row → `crates/email`; the internal
  routes that store and decrypt → `services/authentication_service`.

## Boundaries

- Depends on: AWS KMS (`aws-sdk-kms`), MacroDB.
- Used by: `authentication_service` (encrypts and decrypts),
  `authentication_service_client` and `email` (connection types only).

## Checks

- `cargo test -p generic_mailbox_credentials` (`SQLX_OFFLINE` unset).

## Gotchas

- Only the authentication service may hold a `MailboxSecretCipher`; other
  services ask it per use and drop the secret after one sign-in.
- The cipher uses `MICROSOFT_TOKEN_KMS_KEY_ID` on purpose (no new key or IAM).
  The encryption context's `macro:purpose` keeps the two uses apart; never
  reuse the Microsoft purpose string.
- `Debug` on every connection type prints the protocol only: endpoints, hosts
  and usernames identify a mailbox.
- Deleting a binding does not revoke the secret at the mail server.

## Related

- Rules: `.agents/rules/rust/security.md` (CS-72) · Docs:
  `docs/design/generic-mailbox-providers.md`
