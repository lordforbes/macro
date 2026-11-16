# Task: add read-only generic mailbox providers (JMAP and IMAP) to Macro
(fork of macro-inc/macro)

## Goal
Macro's email service supports Gmail and Outlook (Microsoft Graph) behind
provider-neutral ports. Add two providers, "jmap" and "imap", so a mailbox on
any compliant server can be linked, synced into Macro and read in the normal
inbox UI.
- JMAP (RFC 8620/8621) is the priority. Primary target: Mailflare's JMAP
  endpoint (discovery at /.well-known/jmap). Keep the client generic.
- IMAP is the second adapter, for servers without JMAP.
Phases 1-3 are READ-ONLY. Sending and write-back come later.

## Non-negotiable safety rules (both protocols)
1. Read-only phases must never modify the server.
   - JMAP allowed methods: Core/echo, Mailbox/get, Mailbox/changes, Email/query,
     Email/get, Email/changes, Thread/get, blob download. Nothing else.
   - IMAP allowed commands: CAPABILITY, LOGIN/AUTHENTICATE, LIST, STATUS,
     EXAMINE (never SELECT), UID SEARCH, UID FETCH BODY.PEEK[...] (never plain
     BODY[...], which sets \Seen), LOGOUT.
2. Enforce (1) in types: each protocol client is wrapped in a type exposing
   only the read-only operations, so write methods cannot compile. Add the write
   operations to the wrapper only in the phase that allows them.
3. Never log or trace credentials, message bodies or addresses. Follow the
   repo's convention (Link's Debug prints only id/provider/flags).
4. Credentials are encrypted at rest. Existing auth tests assert no plaintext
   secret storage; they must still pass.
5. JMAP keys: use a dedicated API key. Do NOT use Mailflare's device sign-in
   endpoint (its keys may not expire and may reach every mailbox). If keys can
   be scoped per mailbox, do so.
6. Develop and test only against local servers. No real mailbox until a phase
   passes, then only with a read-only credential.
7. Work on a branch, small commits. Don't refactor Gmail/Outlook code beyond
   adding the new provider variants and match arms.
8. New dependencies must pass cargo-deny. Workspace deps go in the root
   Cargo.toml [workspace.dependencies]. Verify versions, licences and
   maintenance of any JMAP/IMAP/MIME crate before choosing it.

## Phase 0: recon (write design notes BEFORE any code; then STOP for review)
Paths come from PR listings and may have moved. Verify each one.
- crates/email/email_api_client_core: models, ports, services (feature `ports`)
- crates/email/email_api_client_outbound: gmail/, outlook/, features
  outbound-gmail, outbound-outlook. Add outbound-jmap and outbound-imap the
  same way.
- crates/email (domain + Postgres adapters); services/email_service
  (composition root, pubsub workers, mailbox_sync_worker.rs)
- docs/design/outlook-email-support.md and docs/runbooks/outlook-sync.md: READ
  FIRST. Outlook is the template for a non-Gmail provider.
- AGENTS.md / CLAUDE.md in each crate: follow them exactly.
- docs/development/running-locally.md, docker/ compose, flake.nix, justfile
  (`just check`, `just hakari`, SQLx offline metadata, generated clients/SDK).
- Mailflare: docs/api.md, docs/deployment.md, src/lib/jmap/*, the Drizzle schema.
Deliverables:
1. docs/design/generic-mailbox-providers.md: every port the adapters must
   implement; which ports neither protocol can support (contacts, filters,
   signatures, calendar); how unsupported ports fail (typed "unsupported",
   never a panic); how the UI hides them via capability flags; provider-enum
   and migration impact.
2. docs/design/jmap-provider.md: the JMAP methods and capabilities Mailflare
   ACTUALLY implements (Thread/get? Email/set? submission? EventSource?), what
   is missing, rate/CPU considerations.
3. docs/design/imap-provider.md: capability expectations (SPECIAL-USE,
   CONDSTORE/QRESYNC, IDLE, UIDPLUS), folder/flag mapping, threading plan.

## Phase 1: shared foundation (no protocol code yet beyond stubs)
One provider-enum migration, one link model, one credential store, one UI
capability model, shared by both adapters.
- Provider enum: add Jmap and Imap. One additive migration, following the
  Outlook approach for shared tables: concurrent index builds, short lock
  timeouts.
- Generic mailbox link: protocol, endpoint/host/port/TLS mode, username,
  encrypted secret (JMAP: API key; IMAP: password or app password). Create via
  API/CLI first. No UI yet.
- Capability flags per provider (contacts, filters, signatures, calendar, push,
  server threads, multi-label). The UI hides unsupported features, as it does
  for Outlook.
- Shared cursor type: an opaque, versioned per-stream cursor.
  JMAP: per-type state string. IMAP: (UIDVALIDITY, last_uid, optional
  HIGHESTMODSEQ). An invalid cursor (JMAP cannotCalculateChanges, IMAP
  UIDVALIDITY change) triggers a full resync of that stream.
- Shared mapping module with tests:
  * roles: JMAP mailbox roles and IMAP SPECIAL-USE attributes (RFC 6154) ->
    Macro system labels (inbox/sent/trash/spam/drafts); fallback to common
    folder names; other mailboxes become user labels.
  * flags: JMAP keywords and IMAP flags ($seen/\Seen, $flagged/\Flagged,
    $draft/\Draft, $answered/\Answered) -> read/starred/draft/replied.
- Shared thread synthesis module: deterministic provider_thread_id from
  Message-ID / In-Reply-To / References (subject fallback only as a last
  resort); handles two threads merging when a later message links them. IMAP
  always uses it. JMAP uses server threadId when available, else this.
- Shared MIME parsing and attachment path (existing Macro attachment handling).
- Shared read-only safety test harness: snapshot server state before and after
  a sync (message IDs/UIDs, flags/keywords, mailbox counts) and assert identical.
- Feature flags: `outbound-jmap`, `outbound-imap`, both default OFF.
Stop for review at the end of Phase 1.

## Phase 2: JMAP adapter, read-only (priority)
- Discovery via /.well-known/jmap and the session resource; tolerate unknown
  capabilities in `using`; treat unknownMethod as "unsupported".
- Mailboxes via Mailbox/get + changes. Messages via Email/query + Email/get +
  Email/changes. Threads via Email.threadId or Thread/get. Attachments via blob
  download.
- Sync with state strings; never tight-poll (Workers CPU cost). Use EventSource
  only if advertised; otherwise low-frequency polling on state strings.
- Test against a LOCAL Mailflare (`npm run dev`, `npm run db:seed`).
- Done when: seeded mailbox shows in a local Macro inbox with correct threads,
  labels and attachments; safety test passes; `just check` green; generated
  artifacts regenerated.

## Phase 3: IMAP adapter, read-only
- LIST + SPECIAL-USE folder mapping. Per-folder UID cursor; UIDVALIDITY change
  => resync that folder; detect server-side deletions by UID reconciliation;
  backfill with a configurable date window.
- Polling first. IDLE later.
- Connection pool of 2-3 per account, backoff on errors, respect server limits.
- Test against local Dovecot or GreenMail containers.
- Done when: same bar as Phase 2.

## Cross-protocol conformance test (both read-only adapters)
Serve the SAME fixture mailbox through both protocols (for example a local
Stalwart instance, if it exposes both; verify), sync it through each adapter
into Macro, and assert the normalised output matches (messages, labels, flags,
attachments, and threads grouped identically, ignoring thread ID values).
Fixture set: plain text, HTML, multipart/alternative, large attachments,
non-ASCII headers and bodies, missing Message-ID, duplicate Message-ID, long
References chains, out-of-order arrival, expunged/destroyed messages, a cursor
invalidation, messages belonging to several mailboxes (JMAP only).

## Phase 4: sending (explicit approval required before starting)
- JMAP: EmailSubmission (if Mailflare implements it) and Email/set for the
  sent copy.
- IMAP: SMTP submission, plus APPEND to Sent.
- Uncertain-send recovery as Outlook does. Test only against local servers.

## Phase 5: write-back (explicit approval required)
Read/unread, starred, archive, trash, moves, via JMAP Email/set and IMAP
STORE/MOVE/COPY. Pending-label semantics follow the existing label-write
pipeline. Add idempotency and undo.

## Phase 6: UI, push, drafts
Add-account form with protocol choice (JMAP / IMAP). EventSource and IDLE push.
Draft sync.

## Phase 7: upstream
Open an issue first and propose the design. Contact: contribute@macro.com.

## Stop and ask me if
- a required port can't be expressed for either protocol
- the schema change goes beyond additive changes to populated tables
- the local stack won't run
- you're about to touch Gmail or Outlook behaviour
- you want to add any write method to a read-only wrapper
- the adapters need divergent core models (prefer extending the shared layer)