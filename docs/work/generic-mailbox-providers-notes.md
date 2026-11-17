# Working notes for the generic mailbox providers work

Read with TASK-BRIEF.md (the governing plan) and the three design notes. Written 16 Nov 2026, 22:30 UK.

## Where things are

- Repo: /opt/macro/gmp-wt is a git worktree of /opt/macro/src on branch `feat/generic-mailbox-providers`, based on upstream macro-inc/macro `main` at be38105 (16 Nov). One commit so far: e518219 "feat(email): shared cursor, role and flag mapping for generic mailboxes" (Phase 1 slice A, 9 tests, clippy clean).
- Build dir for this worktree: CARGO_TARGET_DIR=/opt/macro/gmp-target (do not use /opt/macro/src/target, the running stack owns it).
- Run everything inside the Nix dev shell, from this worktree. `/usr/local/bin/macro-dev` does `cd /opt/macro/src` first, so it builds, generates and runs `sqlx migrate add` in the running stack's checkout, not here. Use the same `nix develop --command` line from /opt/macro/gmp-wt with the no-AVX2 Bun (/opt/bun-baseline/bin) first on PATH; these CPUs lack AVX2, so stock Bun crashes.
- Test database: a dedicated container `gmp-test-postgres` on 127.0.0.1:55432 (user/password, db `macrodb`, `template1` pre-migrated). Port 5432 is the running stack's database. The flake's shell hook exports DATABASE_URL for 5432, overriding anything set before `nix develop`, so export DATABASE_URL inside the `bash -c` (otherwise `cargo test` runs `#[sqlx::test]` on the stack's server). Never migrate it or point `prepare_db` at it (the root recipe defaults to it; pass `just --set DATABASE_URL postgres://user:password@localhost:55432/macrodb prepare_db`).
- `prepare_db` here also deletes about 98 upstream cache entries (test-only or excluded-crate queries). Restore those (`git checkout` the deleted files) and commit only the entries the change modified.
- `bun run gen-api` needs `OPENAPI_BINS_DIR=/opt/macro/gmp-target/debug`; its later tool-schema step still looks in ./target and fails, which skips the final biome pass. Run `biome check --write --unsafe src/lib/service-clients/` in apps/web afterwards and restore the cognition tool files it deleted.
- Build in offline SQLx mode: `SQLX_OFFLINE=true`. The default checks every query against the live stack database, which fails while the stack is mid-update and must not be loaded by builds anyway. Database tests are a separate step (see docs/development/database.md).
- The running Macro stack (not to be used for tests) is /opt/macro/src on upstream be38105 via systemd `macro.service`, started with `--no-computers` (the computer image cannot be built in time on this VM) and TimeoutStartSec=4h. Status: `~/macro-pilot/08b-restart-and-verify.sh status`. After any update re-run `~/macro-pilot/06-team-and-backup.sh unlock "Delite Social Media Team"`. Backups: `~/macro-pilot/06-team-and-backup.sh backup` to /mnt/backup/macro.
- Spike branch `spike/jmap-adapter` (worktree /opt/macro/jmap-wt): a pre-brief JMAP adapter with wiremock tests. Reference for Phase 2 only; it does not meet the brief (no typed read-only handle, no credential model, tested once against a real mailbox).
- GitHub: no credentials on the VM yet. A push-only deploy key exists at ~/.ssh/macro_fork_deploy; its public half must be added to github.com/lordforbes/macro (Settings > Deploy keys, write access) before `git push`. Fork `main` is 386 commits behind upstream and can fast-forward.
- A real Mailflare JMAP API key sits in /etc/macro/mailflare-jmap.key (root, 600). Per the brief, do NOT use it until Phase 2 passes against a local server. Chris may revoke it.

## Decisions already taken (see generic-mailbox-providers.md section 11)

Widen the four read-path queries; add EmailApiError::Unsupported and MailboxCapabilities; the authentication service stores and decrypts secrets; IMAP identity per imap-provider.md section 4 with a locations table; NOOP and ENABLE allowed in the IMAP read-only set; fork synced before Phase 1.

## Phase 1 remaining (stop for review at the end)

1. Done in Rust (17 Nov): Jmap and Imap in all six Rust provider enums (the sixth is `DbUserProvider` in crates/email/src/outbound/email_pg_repo/db_types.rs), migration 20261116233821 (ADD VALUE IF NOT EXISTS, like the other recent enum migrations; Postgres allows it in a transaction as long as the value isn't used there), every exhaustive arm, strict text conversions, `EmailApiError::Unsupported { capability }` with a `MailboxCapability` enum (HTTP 422), regenerated .sqlx, GraphQL SDL, OpenAPI, SDK and web GraphQL types. Still open: 15 web type errors where the UI assumes Gmail-or-Outlook (prompts, calendar, compose); they need item 3's UI decisions.
2. Done (17 Nov): `email_generic_mailbox_bindings` + `crates/generic_mailbox_credentials`, sealed under the existing Microsoft token KMS key (Chris chose reuse over a new key; envelope scheme, because that key's IAM grant is GenerateDataKey + Decrypt only). Internal auth routes, client methods, email-domain connect (inactive link, cleanup on failure) and the `connect_generic_mailbox` CLI. See design note section 7. Not yet run end to end: needs a local auth service with KMS.
3. Done (17 Nov): `MailboxCapabilities` decided by `email::domain::models::UserProvider::capabilities()`, on the link in GraphQL (`capabilities`) and REST (`Link.capabilities`). Web, per the agreed proposal (mobile too): generic inboxes stay out of the compose From lists, never get Google/Microsoft reconnect or calendar prompts, show a neutral icon, protocol label and "Read-only" chip in Settings › Email, and have their own "Other mailboxes" row in Connections (Manage only). Web type-check and Biome pass.
   Phase 2 preconditions, before any generic link is activated: (a) refuse every write server-side for links without the capability (send, drafts, scheduled send, thread label/read/archive/trash/move, sender block); today those paths route anything not OUTLOOK down the Gmail path. (b) Disable the thread write controls in the web app (row actions, hotkeys, detail view, bulk actions, AI email tools) from `capabilities.organize`/`send`; show disabled, not hidden, on desktop and mobile.
4. Done (17 Nov): `generic/threads.rs`. A thread is named by its root (smallest member without a known parent), so replies never rename it and names do not depend on arrival order; `insert` reports retired ids to move. Persisting the index is Phase 2/3 work.
5. Done (17 Nov): `generic/safety.rs` behind `test-support`: `ServerSnapshot`, `changes`, `assert_read_only`. Phase 2/3 supply a `SnapshotSource` per protocol on its own connection.
6. Done: `outbound-jmap` and `outbound-imap` declared, default off (slice A).

- Web unit tests (vitest) crash with SIGILL on these CPUs under both Bun and Node: a native module in the toolchain needs AVX2. `bunx` also resolves to the Nix (AVX2) Bun; use `bun x`. Type-check with `bun ../../node_modules/typescript-native/bin/tsc --noEmit --skipLibCheck --project tsconfig.json` from apps/web.
- `gen-api` with `OPENAPI_BINS_DIR` skips its build: build the binaries first with `cargo build $(bun scripts/generate-api-schema.ts --print-cargo-bins)`.
- The build directory grew to about 160 GB and filled the disk on 17 Nov; it was deleted with Chris's approval. Watch `df -h /` before long builds, and avoid needless feature-set permutations (each `--all-features` combination is a separate build).
- `macro.service` has been failed since 16 Nov 17:28 UTC: FusionAuth asks for `--fresh` (destructive; Chris's call). Postgres stays up.

## Repo rules that bite

- .agents/rules/rust/*.md and docs/development/database.md are mandatory reading. Highlights: no `std::env::var` (use macro_env_var), tests in a sibling test.rs, `#[expect]` not `#[allow]`, deps via [workspace.dependencies] then `just hakari`, queries via sqlx::query! with .sqlx regenerated by `just prepare_db` from the root, migrations only via `sqlx migrate add`.
- Pre-commit scans staged diffs for secrets and PII; commit as author lordforbes <lordforbes@users.noreply.github.com>.
- Never touch Gmail or Outlook behaviour beyond adding provider variants and match arms; never test against a real mailbox; ask before any schema change beyond additive ones.

## Working method

Chris is steering from the Claude app. Keep commits small and messages plain. Report what was verified (test counts, clippy) rather than what was written.
