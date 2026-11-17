# Local test server for generic mailboxes

A disposable local Mailflare (JMAP) server, seeded with the fixture set from
the generic mailbox brief, for the adapter's live tests. Never point these
scripts or tests at a real mailbox.

## Start Mailflare

Build the image from Mailflare at the commit the adapter was checked against:

```sh
git clone https://github.com/hieunc229/mailflare && cd mailflare
git checkout f4a17bd
docker build -t mailflare:f4a17bd .
```

Run it on this machine's loopback only:

```sh
docker run -d --name gmp-mailflare --network host \
  -e HOST=127.0.0.1 -e PORT=3110 \
  -e SMTP_INBOUND_PORT=2526 -e SMTP_INBOUND_HOST=127.0.0.1 \
  -e APP_URL=http://localhost:3110 -e MAIL_HOSTNAME=localhost \
  -v gmp-mailflare-data:/data mailflare:f4a17bd
```

Why these flags:

- `--network host` with `HOST` and `PORT`: Mailflare's Node build redirects
  `/.well-known/jmap` to the address it listens on, not to `APP_URL`, while
  the session uses `APP_URL`. The adapter refuses a redirect to another
  origin, so the listen address and `APP_URL` must be the same origin.
- `SMTP_INBOUND_HOST`: the SMTP listener ignores `HOST` and binds every
  interface without it.
- No `SMTP_URL`: the server cannot send mail anywhere.

## Provision and seed

```sh
python3 -I provision_mailflare.py /path/to/credentials.json
# Mark the test domain active: without DNS or a receiving provider Mailflare
# leaves it pending and drops inbound mail ("No routing for inbound address").
docker exec gmp-mailflare node -e "const D=require('better-sqlite3');new D('/data/mailflare.sqlite').prepare(\"update domains set status='active' where hostname='example.com'\").run()"
python3 -I seed_fixtures.py
```

The fixtures: plain text, HTML, multipart/alternative, non-ASCII headers and
body, a 5 MiB and a small attachment, a missing Message-ID, two messages
sharing a Message-ID, and a six-message reply chain with a long References
chain delivered newest first.

## Run the live test

```sh
JMAP_LIVE_CREDENTIALS=/path/to/credentials.json \
  cargo test -p email_api_client --features outbound-jmap,test-support \
  --lib jmap::test::live -- --ignored
```

It syncs every folder inside the read-only safety check and fails if the
server changed. Reset with `docker rm -f gmp-mailflare && docker volume rm
gmp-mailflare-data`.
