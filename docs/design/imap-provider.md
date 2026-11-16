# IMAP provider: design notes

Status: Phase 0 recon notes for review, 16 November 2026. Read with `generic-mailbox-providers.md`. IMAP is the second adapter (Phase 3), for servers without JMAP. Nothing here has been tested against a server yet; items marked **verify** must be checked in Phase 3 against local Dovecot or GreenMail.

## 1. Capability expectations

| Capability | Required | Use |
| --- | --- | --- |
| IMAP4rev1 (RFC 3501), TLS on 993 or STARTTLS | Yes | Baseline. Plain-text connections are refused. |
| UIDPLUS (RFC 4315) | No | Only matters for writes (Phase 5). |
| SPECIAL-USE (RFC 6154) | Preferred | Folder roles. Fallback: name table. |
| CONDSTORE / QRESYNC (RFC 7162) | Optional | Cheap flag-change and deletion detection. Without it: periodic UID and flag reconciliation. |
| OBJECTID (RFC 8474) | Optional | Stable message and thread ids across folders. Changes the identity plan in section 4 when present. |
| IDLE (RFC 2177) | Optional | Push, Phase 6. Polling first. |
| LIST-STATUS, ESEARCH, MOVE | Optional | Fewer round trips; MOVE for Phase 5. |

Authentication: LOGIN or AUTHENTICATE PLAIN with a password or app password. OAuth (XOAUTH2) is out of scope; Gmail and Microsoft mailboxes should use their native providers.

## 2. Read-only command set (safety rules 1 and 2)

Allowed: `CAPABILITY`, `LOGIN`/`AUTHENTICATE`, `LIST`, `STATUS`, `EXAMINE`, `UID SEARCH`, `UID FETCH` with `BODY.PEEK[...]` and metadata items, `LOGOUT`. Also `NOOP` and `ENABLE` (for CONDSTORE/QRESYNC), which change no mailbox state; adding these two to the brief's list needs approval.

Never: `SELECT` (use `EXAMINE`, which opens read-only and cannot set `\Seen` or clear `\Recent`), plain `BODY[...]`, `STORE`, `APPEND`, `EXPUNGE`, `CLOSE` on a selected mailbox, `COPY`, `MOVE`, `CREATE`, `DELETE`, `RENAME`, `SUBSCRIBE`.

Enforcement: the adapter holds a read-only handle with one method per allowed command. `uid_fetch_peek` takes a typed list of items that can only render `BODY.PEEK[...]`, `FLAGS`, `INTERNALDATE`, `RFC822.SIZE`, `ENVELOPE`, `BODYSTRUCTURE`, `UID`, `MODSEQ`. A test renders every item variant and asserts none produces `BODY[` without `.PEEK`. The safety harness additionally checks `\Seen` and `\Recent` are unchanged after a sync.

## 3. Client library

Candidate: `async-imap` 0.11.2 (MIT OR Apache-2.0, tokio runtime feature, maintained by the Delta Chat developers, built on `imap-proto`). Not yet run through this workspace's `cargo deny`; do that before adding it, and confirm the latest version at that time.

It exposes `select`, `store`, `append` and the rest on its session type, so it must sit behind the private transport layer, never in the adapter's public surface. If wrapping proves leaky, the alternative is a small purpose-built client on `imap-codec`/`imap-next`; decide after a Phase 3 spike.

MIME parsing: `mailparse` 0.16.1, already a dependency of this crate. No new MIME crate.

## 4. Message identity (the hard part)

Macro's engine keys a message by one provider id per mailbox link and expects that id to survive a move between folders. IMAP's native identity is `(folder, UIDVALIDITY, UID)`, which changes when a message moves.

Plan:

1. If the server has OBJECTID: provider id is `EMAILID`. Moves keep identity.
2. Otherwise: provider id is derived from the normalised `Message-ID` header plus a short hash of `INTERNALDATE` and `RFC822.SIZE`, so two different messages sharing a Message-ID stay distinct, and a moved message (same bytes, same internal date on well-behaved servers) keeps its id. A per-link table maps provider id to its current `(folder, UIDVALIDITY, UID)` locations.
3. No Message-ID: hash of selected headers, internal date and size.

Consequences to accept or revisit at review: the same message copied into two folders is one Macro message with two locations (its folder shown is the highest-priority role); servers that rewrite `INTERNALDATE` on move will show a move as delete plus new message. Both are **verify** items against Dovecot.

This needs one small table of locations per link. If the review prefers not to add it, the fallback is "one Macro message per folder copy", which is simpler and shows duplicates.

## 5. Folders and flags

- `LIST "" "*"` with SPECIAL-USE attributes: `\Inbox` (or the name INBOX), `\Sent`, `\Drafts`, `\Trash`, `\Junk`, `\Archive` to `FolderRole`. `\All` and `\Flagged` are virtual views and are skipped. `\Noselect` folders are listed as parents only.
- Name fallback when SPECIAL-USE is absent: a table of common names per role (Sent, Sent Items, Sent Messages; Trash, Deleted Items, Deleted Messages; Junk, Spam, Junk E-mail; Drafts; Archive, Archives), case-insensitive, under any namespace prefix. Ambiguity resolves to `Other`.
- Hierarchy from the server's delimiter. Folder provider id is the UTF-7-decoded full name; a rename therefore looks like a new folder and triggers a resync of that folder. With OBJECTID, `MAILBOXID` is used instead.
- Flags: `\Seen` to read, `\Flagged` to flagged, `\Draft` to draft, `\Answered` to replied (if that fact is added to the shared state), `\Deleted` treated as removed. Keywords are ignored at first.

## 6. Sync

Cursor per folder: `v1:imap:<uidvalidity>:<last_uid>[:<highestmodseq>]`.

- Open with `EXAMINE`. If `UIDVALIDITY` differs from the cursor, return `OutdatedCursor`; the engine resets that folder's stream.
- New mail: `UID FETCH <last_uid+1>:*` of metadata, in pages.
- Flag changes and deletions:
  - With QRESYNC/CONDSTORE: `CHANGEDSINCE` and `VANISHED` from the stored `HIGHESTMODSEQ`.
  - Without: reconcile on a slower cadence. `UID SEARCH ALL` (or `UID FETCH 1:* (FLAGS)` in ranges) compared with stored locations; missing UIDs are removals, changed flags are changes.
- Initial backfill: newest first, by a configurable date window (`UID SEARCH SINCE`), then older windows until done. Progress has no reliable total; the engine already allows unknown totals.
- Message read (`message`): `UID FETCH … BODY.PEEK[]` and parse locally; attachments are MIME parts, addressed by part path, fetched with `BODY.PEEK[<part>]`.
- Threads: always the shared synthesis. Server `THREAD` extensions are not used, so results match the JMAP fallback.

## 7. Connections and limits

- A pool of 2–3 connections per account, one folder examined per connection at a time, idle connections closed after a few minutes.
- Exponential backoff on connection and authentication errors; an authentication failure marks the link as needing new credentials rather than retrying, to avoid account lockouts.
- Polling interval per folder: Inbox about every minute while the mailbox's user is active, other folders every several minutes; much slower when idle. Respect `[THROTTLED]` and connection-limit responses.
- IDLE in Phase 6, on the Inbox only at first.

## 8. Testing

- Unit: scripted server transcripts for the read-only handle, cursor parsing, UIDVALIDITY change, role and flag mapping, identity derivation.
- Local: Dovecot (primary) and GreenMail containers with seeded Maildirs; the read-only safety harness around a full sync.
- Cross-protocol conformance against the JMAP adapter using one fixture mailbox served over both protocols. Stalwart is the candidate server; that it serves the same mailbox over JMAP and IMAP with consistent threading inputs is **verify**.

## 9. Open points

1. Approve `NOOP` and `ENABLE` in the read-only command set.
2. Approve the identity plan in section 4 and its locations table, or choose per-folder copies.
3. Library: `async-imap` behind a wrapper, pending cargo-deny and a spike.
4. Plain-text and self-signed TLS: refuse both by default; a per-link "trust this certificate" option is a later decision.
