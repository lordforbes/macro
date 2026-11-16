# JMAP provider: design notes

Status: Phase 0 recon notes for review, 16 November 2026. Read with `generic-mailbox-providers.md`.

Target: any RFC 8620/8621 server. First server: Mailflare (`hieunc229/mailflare`), read at commit `f4a17bd` (14 Nov 2026). Facts below marked **source** come from that code, **docs** from its `docs/api.md`, **observed** from one read-only session against a deployed instance on 16 Nov 2026 (see the shared note, section 10).

## 1. What Mailflare actually implements

| Area | Finding | Basis |
| --- | --- | --- |
| Discovery | `/.well-known/jmap` answers 301 to `/jmap/session` on the same origin. The client must follow one same-origin redirect and refuse any other. | observed |
| Auth | `Authorization: Bearer <api key>`, or HTTP Basic with the key as the password. Key needs the `jmap` scope. Minted under Settings > Account > App passwords. | docs, observed |
| Capabilities | `urn:ietf:params:jmap:core`, `:mail`, `:submission`. No contacts, calendars, sieve, vacation response, quota or websocket. | source |
| Account | One account per user; account id is the user id. | docs |
| Mailbox methods | `Mailbox/get`, `/query`, `/changes`, `/set` (folders only). No `Mailbox/queryChanges`. | source |
| Email methods | `Email/get`, `/query`, `/changes`, `/queryChanges`, `/set`, `/import`. No `Email/copy`, `Email/parse`. | source |
| Threads | `Thread/get`, `Thread/changes`; `Email.threadId` is populated. | source, observed |
| Other | `Identity/get`, `EmailSubmission/set`, `SearchSnippet/get`, `Core/echo`. | source, docs |
| Push | `eventSourceUrl` is advertised. The server implements it by re-reading state every `ping` seconds (default 30, minimum 10) for as long as the connection is open. | source |
| Blobs | Download and upload through the session templates. Attachment blobs `att~<id>`; message blobs `msg~<id>`. A message blob is the stored original for imported mail and a rebuilt minimal MIME for other mail. | source |
| Limits | 32 calls per request, 200 objects per get, 100 per set, 250 ids per query, 10 MiB request, 4 concurrent requests. | source |

### Things that differ from a textbook server

1. **One mailbox per email.** `maxMailboxesPerEmail` is 1. Each Mailflare mailbox is a top-level JMAP Mailbox whose children are the six role folders and the user folders; a message is in exactly one child. The top-level Mailbox answers queries with all of its mail but no message lists it in `mailboxIds`. The adapter must not enumerate top-level mailboxes as folders, or every message is read twice.
2. **Query window.** `Email/query` only ever orders the newest 10,000 matches. A folder larger than that cannot be enumerated with one filter. The adapter must window by `before`/`after` date.
3. **Change log, and a conflict between docs and code.** `docs/api.md` says every `*/changes` returns `cannotCalculateChanges`. The code at `f4a17bd` keeps a change log, answers `Email/changes`, and returns `cannotCalculateChanges` only for states older than its retention or with a different access fingerprint. The deployed instance answered `Email/changes` normally. The adapter must handle both: use `/changes`, and on `cannotCalculateChanges` reset the stream and re-enumerate. Which behaviour a given deployment has depends on its version.
4. **State strings** are one value shared by Email, Mailbox and Thread, of the form `c1:<sequence>:<access fingerprint>`. Treat as opaque.
5. **Keywords.** Only `$seen`, `$flagged` and `$draft` are stored. No `$answered`, no user keywords.
6. **No per-message version.** JMAP has no ETag per object. `MailboxMessage.version` is `None`; change detection relies on the change log.
7. **No "sent by me" fact.** The Sent role is the only direct evidence.
8. **Large attachments** above about 3 MB on outbound mail are replaced by Mailflare with expiring public share links. For read-only sync this only affects how such messages look; it matters in Phase 4.

### Keys (safety rule 5)

- Use an App-password API key with only the `jmap` scope. Never the device sign-in endpoint.
- The key table has a per-key mailbox restriction (`mailboxScopeEnabled` with a key-to-mailbox table) and the state string carries an access fingerprint, which suggests a key can be limited to chosen mailboxes. Whether the App-password screen exposes this, and whether JMAP honours it for every method, is **unverified**. Verify on a local instance in Phase 2; if it works, require it.
- Whether these keys expire is unverified.

## 2. Client shape

No JMAP crate. The protocol is JSON over HTTPS, and `reqwest`, `serde_json` and `url` are already dependencies of this crate, so nothing new goes through cargo-deny. The known Rust JMAP client crate exposes write methods, which would undercut the type-level read-only rule.

Layers (see shared note, section 4):

- Private transport: session discovery, one POST per request, error mapping, blob GET. Redirects off; one same-origin discovery redirect followed by hand. `apiUrl` and the expanded `downloadUrl` must be on the configured origin, or the session is rejected, so a key is never sent elsewhere.
- Public read-only handle: `core_echo`, `mailbox_get`, `mailbox_changes`, `email_query`, `email_get`, `email_changes`, `thread_get`, `download_blob`. Typed arguments; no free-form method name.
- `using` lists only what a call needs. Unknown capabilities in the session are ignored. A method-level `unknownMethod` maps to `Unsupported`.

Error mapping: HTTP 401 to `AuthRequired`; 403 to `Forbidden`; 429 to `RateLimited` with `Retry-After`; 5xx to `Transient`. Method errors: `cannotCalculateChanges` and `anchorNotFound` to `OutdatedCursor`; `accountNotFound`, `forbidden` to `Forbidden`; `serverFail`, `serverUnavailable` to `Transient`. Error text from the server is never logged beyond a short alphanumeric type token.

## 3. Mapping to the mailbox ports

| Port method | JMAP |
| --- | --- |
| `folders` | `Mailbox/get` (id, name, parentId, role). Roles map through the shared table. |
| `folder_changes`, first round | Read the Email state, then `Email/query` with `inMailbox`, newest first, paged by `anchor` + `anchorOffset: 1` (stable when messages are deleted mid-enumeration), in date windows when a folder exceeds the server's query window. Checkpoint is the state read at the start, so changes during enumeration are replayed. |
| `folder_changes`, later rounds | `Email/changes` from the checkpoint, then `Email/get` of `mailboxIds` for created and updated ids. In this folder: changed. Elsewhere, destroyed or not found: removed from this stream. The engine re-reads each to decide. |
| `message` | `Email/get` with headers, addresses, `textBody`/`htmlBody` parts and `bodyValues`, `attachments`. HTML goes through the existing sanitiser. |
| `organization` | `Email/get` of `mailboxIds`, `keywords`. |
| `recent_inbox_messages` | `Email/query` on the Inbox, then `Email/get` without bodies. |
| `attachment` | Blob download; the attachment's provider id is its `blobId`. |
| `mime` | `Email/get` `blobId`, then blob download. |

Because JMAP changes are account-wide, every folder stream reads the same change log. With six to a few dozen folders that is acceptable but wasteful; a later improvement is one account-level stream that fans out.

Threads: use `Email.threadId`. If a server omits it, fall back to the shared synthesis.

## 4. Polling, push and cost

Mailflare runs on Cloudflare Workers, where each request and each second of an open EventSource costs CPU and database reads on the mailbox owner's account.

- Poll with one cheap request per mailbox per interval: read the Email state (`Email/get` with an empty id list) and do nothing more if it is unchanged. Default interval 60 seconds while a user of that mailbox is active in Macro, several minutes otherwise. Never poll in a tight loop.
- Do not hold EventSource connections open in the read-only phases. Server-side it is the same polling, held open per client. Revisit in Phase 6.
- Initial import: pages of at most 200 ids, one message fetch per reconciliation as the engine already does, bounded concurrency (the server advertises 4 concurrent requests; use 2).

## 5. Testing

- Unit: a mock JMAP server (wiremock, already a dev-dependency) for discovery, redirects, origin checks, paging, change classification, cursor invalidation, normalisation.
- Local: a Mailflare instance on the VM (Docker self-host or `npm run dev` + `npm run db:seed`), a local Macro, the read-only safety harness around a full sync.
- Fixtures: the cross-protocol list in the brief, plus a message in a user folder, a move between folders, and a folder over the query window (seeded).
- No real mailbox until Phase 2 passes locally, then only with a read-scoped key.

## 6. What the spike showed, and what it lacks

The pre-brief spike (reference only) confirmed against one real mailbox that discovery, folder listing, anchor-paged enumeration, `Email/changes` from a checkpoint, message fetch and raw message download all behave as the code suggests. Attachments were not exercised. It lacks: the typed read-only handle, skipping of top-level mailboxes, date windowing, `Unsupported` handling, and any integration with links, credentials or the engine.

## 7. Open points

1. Per-mailbox key scoping and key expiry in Mailflare (verify locally).
2. Whether to treat Mailflare's top-level Mailbox generically ("a mailbox with children that no message lists") or with a Mailflare-specific rule. Proposed generic rule: do not open a stream for a mailbox whose `totalEmails` is not backed by any message listing it; simpler alternative is to skip mailboxes with children and no role. Needs a decision.
3. Mailflare is young and its docs lag its code. Pin the tested commit in the test setup and re-run the conformance suite when it updates.
