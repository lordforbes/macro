# Generic mailbox providers (JMAP and IMAP): shared design

Status: Phase 0 recon notes for review, 16 November 2026. No code is proposed for merge yet. Companion notes: `jmap-provider.md`, `imap-provider.md`.

Checked against the fork at `276577e` (12 Nov 2026) and upstream `macro-inc/macro` at `3a55270` (16 Nov 2026). Mailflare was read at `f4a17bd` (14 Nov 2026).

## 1. What was read, and corrections to the brief

Read: `crates/email_api_client/AGENTS.md`, `crates/email/AGENTS.md`, the root `AGENTS.md` build rules, `docs/design/outlook-email-support.md` (the solution, credentials, genericity audit and migration sections in full; the calendar and delivery sections skimmed), the mailbox ports and models, the sync engine, the Outlook adapter, worker composition, mailbox initialisation, and Mailflare's `docs/api.md` and `src/lib/jmap/*`.

Not yet read in full: `docs/runbooks/outlook-sync.md`, `docs/development/running-locally.md`, `docs/development/database.md`, `.agents/rules/rust/*`. These must be read before Phase 1 code.

Corrections to the brief's paths (same in fork and upstream):

| Brief says | Actual |
| --- | --- |
| `crates/email/email_api_client_core` | Does not exist. Contracts are in `crates/email_api_client/src/domain` (feature `ports`). |
| `crates/email/email_api_client_outbound` | Does not exist. Adapters are in `crates/email_api_client/src/outbound/{gmail,outlook}` behind `outbound-gmail` and `outbound-outlook`. |
| "The UI hides unsupported features, as it does for Outlook" | Not how Outlook works. See section 6. |

The fork is four days behind upstream. Sync it before branching, because the email crates change daily.

## 2. Where the new providers attach

Macro has two generations of provider contract in the same crate:

1. The Gmail-shaped traits in `domain/ports.rs` (`MailboxSyncClient`, `MailboxMessageClient`, `MailboxLabelClient`, …) driven by `EmailApiClientServiceImpl`. Label-based, history-id cursors.
2. The mailbox engine added for Outlook: traits in `domain/ports/mailbox.rs`, models in `domain/models/provider.rs`, the durable sync engine in `crates/email/src/domain/mailbox.rs`, storage in `crates/email/src/outbound/mailbox_pg*`, workers in `services/email_service/src/mailbox_sync_worker.rs`.

JMAP and IMAP attach to the second. Its model is folders with roles, one change stream per folder with an opaque cursor, a reconciliation queue that re-reads each changed message, and normalised state facts (read, flagged, draft, inbox, trash, junk). Both protocols are folder-based, so this fits without new core models.

The service-side gateway `ProviderMailboxGateway<R, T, L>` (`services/email_service/src/outbound/email_api/microsoft.rs`) is already generic over the adapter.

## 3. Ports

Every trait in `domain/ports/mailbox.rs`, and what each protocol does with it.

| Port (methods) | Needed for read-only | JMAP | IMAP |
| --- | --- | --- | --- |
| `MailboxContentReader` (`folders`, `message`, `organization`, `recent_inbox_messages`, `attachment`, `mime`) | Yes | Mailbox/get, Email/get, blob download | LIST, UID FETCH BODY.PEEK |
| `FolderChangeReader` (`folder_changes`) | Yes | Email/query then Email/changes | UID SEARCH / UID FETCH by cursor |
| `MailboxTokenSource` (`access_token`) | Yes | Returns the decrypted API key | Returns the decrypted password |
| `ScopedMailboxRepository`, `MailboxRequestCount`, `MailboxRequestGate` | Yes (trivial) | No gate at first; count unknown | Connection pool is the gate |
| `MailboxWatchClient` (`watches`, `create_watch`, `renew_watch`, `remove_watch`) | No. Polling until Phase 6 | EventSource later | IDLE later |
| `MailboxActionWriter` (`apply_message_action`, `submit_draft`) | No. Phase 4–5 | Email/set, EmailSubmission/set | STORE/MOVE, SMTP + APPEND |
| `MailboxDraftClient` (11 methods) | No. Phase 6 | Email/set, upload | APPEND to Drafts |
| `MailboxSettingsClient` (`category_messages`, `sender_rules`, `create_sender_rule`, `remove_sender_rule`) | Unsupported | No equivalent in RFC 8621 (Sieve is separate) | No equivalent |
| `MailboxLabelClient` (legacy trait; categories/tags) | Unsupported at first | Keywords exist, but Mailflare stores only `$seen`, `$flagged`, `$draft` | Keywords are server-dependent |
| `MailboxAddressBookReader` (`contact_folders`, `contact_changes`, `self_contact`, `contact_photo`) | Unsupported | JMAP Contacts is a separate RFC; Mailflare does not serve it | No equivalent |
| `MailboxCalendarClient` and the Outlook calendar module | Unsupported | No | No |
| `MailboxRejectedTokenRefresh` | Not applicable | Static key | Static password |

### How unsupported ports fail

`EmailApiError` has no "unsupported" variant today (`RateLimited`, `AuthRequired`, `Forbidden`, `NotFound`, `Conflict`, `OutdatedCursor`, `Transient`, `CredentialsUnavailable`, `Permanent`). Proposal: add `EmailApiError::Unsupported { capability }` to the shared layer. Adapters return it; they never panic and never return a silent empty success for a write. Read-shaped calls that the UI makes unconditionally (labels list, blocked senders) return an empty list at the gateway, decided from the capability set in section 6, without calling the adapter.

Preferred shape: the new adapters do not implement the unsupported traits at all, so the compiler rejects a call path that would reach them. This needs the per-provider `match` sites (section 5) to answer for the new providers without calling an adapter.

## 4. Read-only enforced in types (safety rule 2)

Each protocol client is split in two layers inside its adapter module:

- A private transport that can send anything.
- A public read-only handle exposing one method per allowed operation. The adapter holds only this handle.

JMAP: the handle's methods are `core_echo`, `mailbox_get`, `mailbox_changes`, `email_query`, `email_get`, `email_changes`, `thread_get`, `download_blob`. There is no method taking a method name as a string. A unit test asserts the set of method names the handle can emit equals the brief's allowlist.

IMAP: the handle exposes `capability`, `login`, `list`, `status`, `examine`, `uid_search`, `uid_fetch_peek`, `logout`. `uid_fetch_peek` takes a typed section list that can only render `BODY.PEEK[...]`. `SELECT`, `STORE`, `APPEND`, `EXPUNGE`, `MOVE`, `COPY` have no method.

Write operations are added to these handles only in the phase that allows them.

The shared read-only safety harness snapshots server state before and after a full sync (ids or UIDs, flags or keywords, per-mailbox counts, `\Seen` state) and asserts equality.

## 5. Provider enum and migration impact

Adding `Jmap` and `Imap` touches:

| What | Where | Count |
| --- | --- | --- |
| Postgres enum `email_user_provider_enum` | new migration: `ALTER TYPE … ADD VALUE` (one statement per value; a new value cannot be used in the transaction that adds it, so these are `-- no-transaction` migrations) | 1–2 files |
| Rust enums | `crates/email/src/domain/models/link.rs`, `crates/models_email/src/email/{api,service}/link.rs`, `crates/email_db_client/src/links/types.rs` (`DbUserProvider`, with a `Doppleganger` mirror), `crates/graphql_email/src/user_objects.rs` (`GraphqlEmailProvider`) | 5 |
| Text-to-enum conversions | `mailbox_pg/lifecycle.rs`, `mailbox_pg/settings.rs` (2), `email_pg_repo/draft_attachments.rs`, `email_pg_repo/draft_transfer.rs`, `email_service/src/outbound/inbox_health.rs`, `email_service/src/api/middleware/link.rs` | 7 |
| Exhaustive `match` arms | attachment access, recent inbox, message source, inbox health token, mailbox settings (3), draft size limits (2), health refresh, catalog calendar flag, blocked senders, resync, DB conversions | about 26 |
| SQL filtering on `l.provider = 'OUTLOOK'` | about 35 Rust files under `crates/email/src/outbound/mailbox_pg*`, `email_service/src/outbound/*` | see below |
| Generated artefacts | `.sqlx/` offline query data (`just prepare_db`), GraphQL schema and generated TypeScript clients, OpenAPI | regenerate |
| Frontend | at least `apps/web/src/features/settings/Email.tsx`, `connections/integration-items.tsx`, compose attachment limits | to audit in Phase 1 |

Checked: all seven text conversions are strict. Unknown text is an error; none falls back to Gmail. So a row with a new provider value cannot be picked up by Gmail workers through these paths. Gmail-side SQL that selects links without a provider filter still needs auditing.

### The engine is generic in types but Outlook-specific in data

This is the main finding and the main risk.

- Sync claims (`mailbox_pg.rs` stream and message claims, `mailbox_pg/catalog.rs`) filter on `l.provider = 'OUTLOOK'`.
- Credential health (`mailbox_pg/credentials.rs`) writes `email_link_microsoft_scopes` and custodian scope rows on every token read.
- Mailbox initialisation (`email_service/src/outbound/mailbox_init.rs`) consumes a Microsoft OAuth grant and takes an `outlook:{tenant}:{mailbox}` identity lock.
- Worker composition names `OutlookApiClientRepository` concretely and gates the whole engine on `OUTLOOK_SYNC_ENABLED`.
- Telemetry span and log names (`outlook.sync.*`, "outlook import completed") are a contract read by monitors.

Options:

- **A. Broaden the shared queries** to `provider IN ('OUTLOOK','JMAP','IMAP')` for the read path only (catalog, stream claim, message claim, progress), add a routing gateway that picks the adapter by the link's provider, and give generic links their own credential source and initialiser. Smallest change. It edits SQL that Outlook runs, without changing what Outlook does.
- **B. Parallel queries and workers** for generic providers, leaving every Outlook query byte-identical. No shared SQL edits, but duplicates claim logic that must then be kept in step.

Recommendation: A, limited to the four read-path queries, each change reviewed. This is a "touching Outlook code" item under the brief's stop rules, so it needs explicit approval. Outlook-only subsystems (watches, contacts, settings work, commands, drafts, calendar) keep their `'OUTLOOK'` filter and so ignore generic links with no change.

New deployment switches, default off: `GENERIC_MAILBOX_CONNECTIONS_ENABLED`, `GENERIC_MAILBOX_SYNC_ENABLED`.

Migration style follows the Outlook document: short metadata transactions with a one-second `lock_timeout`, new empty tables with ordinary index builds, any index on a populated shared table built `CONCURRENTLY` in a single-statement `-- no-transaction` migration. Phase 1 as proposed adds enum values and one new empty table; it adds no column or index to a populated table.

## 6. Capabilities and the UI

The brief assumes an existing mechanism by which the UI hides features Outlook lacks. There is none. The Outlook design states the opposite policy: parity, "do not declare parity by hiding a Gmail feature", with capability errors made explicit. The link API object carries only narrow flags such as `needs_calendar_permission`.

So a capability model is new work, not reuse. Proposal:

- A `MailboxCapabilities` value derived server-side from the provider (and later refined from the JMAP session or IMAP CAPABILITY): `send`, `organize` (read/flag/move), `drafts`, `push`, `contacts`, `sender_rules`, `tags`, `calendar`, `server_threads`, `multi_mailbox_messages`.
- Exposed on the link in GraphQL and REST. Gmail and Outlook report everything they do today, so their UI does not change.
- The frontend hides or disables controls from this value. The domain checks stay authoritative: a write to a read-only mailbox is refused server-side with `Unsupported` whatever the client shows.

For read-only phases a generic mailbox reports every write capability false. The compose, reply, archive, mark-read and trash controls therefore need a disabled state. That is frontend work in Phase 1–2, earlier than the brief's Phase 6 UI item.

## 7. Link model and credential store

One new table, empty at creation:

`email_generic_mailbox_bindings(link_id uuid primary key references email_links on delete cascade, protocol, endpoint (JMAP session URL origin), host, port, tls_mode, username, secret_ciphertext bytea, kms_key_id, encryption_version smallint, created_at, updated_at)` with checks that the right columns are set per protocol and that the ciphertext is not empty.

Encryption follows the existing precedent in `crates/cursor_api_key/src/cipher.rs`: direct `kms:Encrypt` (the secret is far below KMS's 4 KB limit), an encryption context naming the purpose and the link id so a row copied to another link fails to decrypt, a redacting, zeroising secret type. The self-hosted stack already runs LocalStack, which provides KMS.

Open question for review: who decrypts. The Outlook rule is that the authentication service owns refresh tokens and workers only receive short-lived access tokens. A static API key or IMAP password has no short-lived form, so either (a) the authentication service stores and returns it per request through the existing internal client, or (b) the email service decrypts it itself. (a) matches the existing ownership rule and keeps the "no plaintext secret storage" tests in one place; it is the recommendation.

`email_links` gets a row with the new provider, `grant_id` null, and no custodian or Microsoft scope rows. Identity for de-duplication is `(protocol, endpoint or host, username)`. Links are created through an internal API or CLI first; no UI.

Logging: `Debug` for the binding prints link id, protocol and flags only. Endpoint, host and username are treated as addresses and not logged.

## 8. Shared modules

- **Cursor.** The engine already stores an opaque `StreamToken` per stream. Generic cursors are versioned text: `v1:jmap:…`, `v1:imap:<uidvalidity>:<last_uid>[:<highestmodseq>]`. An unreadable or invalid cursor returns `EmailApiError::OutdatedCursor`, which the engine already turns into a stream reset.
- **Roles.** JMAP `role` and IMAP SPECIAL-USE (RFC 6154) to `FolderRole` (`Inbox`, `Sent`, `Drafts`, `Archive`, `Trash`, `Junk`, `Other`), with a name fallback table. Note: this engine models folders, not Gmail-style system labels; other mailboxes become folders, not user labels. The brief's "user labels" wording does not match the engine and should be read as "ordinary folders".
- **Flags.** `$seen`/`\Seen`, `$flagged`/`\Flagged`, `$draft`/`\Draft` to `MailboxState`. `$answered`/`\Answered` has no field in `MailboxState` today; adding one is a shared-model extension to decide in Phase 1.
- **Threads.** A deterministic `provider_thread_id` from Message-ID, In-Reply-To and References, with merge handling. IMAP always uses it. JMAP uses the server `threadId` when present.
- **MIME.** Reuse `mailparse` 0.16.1, already a dependency of this crate for Gmail, and `email_utils` sanitising. No new MIME crate.
- **Sent detection.** Neither protocol has a "sent by this mailbox" flag. Use the Sent role, and compare the From address with the mailbox address for copies filed elsewhere.

## 9. Local test environment (safety rule 6)

- Mailflare: its repository documents a Docker self-host (`docs/self-hosting.md`) and `npm run dev` with `npm run db:seed`. Run one on the Macro VM.
- IMAP: Dovecot or GreenMail container.
- Conformance: Stalwart is the candidate for serving one fixture mailbox over both protocols. Not verified yet.

## 10. State of earlier work, stated plainly

Before this brief existed, a JMAP adapter spike was written and committed on the VM (branch `jmap-provider`, two unpushed commits) and run once, read-only, against a real Mailflare mailbox. It used only allowlisted methods and modified nothing. It does not meet this design: its transport accepts any method name, it has no capability or credential model, and it was tested against a real mailbox. It is a reference for Phase 2, not a deliverable. The real key is still in a root-only file on the VM and should be revoked or removed.

## 11. Decisions (review of 16 November 2026, Chris)

All recommendations in these notes were accepted.

1. **Shared queries:** option A. Widen the four read-path queries (catalog claim, stream claim, message claim, progress) to accept the new providers. No other Outlook SQL changes.
2. **Shared-layer additions:** add `EmailApiError::Unsupported` and a `MailboxCapabilities` model exposed on the link; do the frontend work to disable write controls for read-only mailboxes in Phase 1–2.
3. **Secrets:** the authentication service stores and decrypts generic mailbox secrets; the email service asks it per use.
4. **IMAP identity:** the scheme in `imap-provider.md` section 4, with the per-link locations table.
5. **IMAP commands:** `NOOP` and `ENABLE` are allowed in the read-only set.
6. **Fork:** sync with upstream before Phase 1. On 16 Nov the fork's `main` was 386 commits behind and can fast-forward.

Still open, not blocking Phase 1: whether `$answered` becomes a shared state fact (default: not in Phase 1); the fate of the real Mailflare key on the VM. The spike branch is kept as `spike/jmap-adapter`.
