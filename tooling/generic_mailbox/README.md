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

Run it on the local stack's services network, so the stack's workers reach
it by name, with its ports published on this machine's loopback only:

```sh
docker run -d --name gmp-mailflare --hostname mailflare \
  --network macro_services --network-alias mailflare \
  -p 127.0.0.1:3110:3110 -p 127.0.0.1:2526:2526 \
  -e HOST=mailflare -e PORT=3110 \
  -e SMTP_INBOUND_PORT=2526 -e SMTP_INBOUND_HOST=mailflare \
  -e APP_URL=http://mailflare:3110 -e MAIL_HOSTNAME=mailflare \
  -v gmp-mailflare-data:/data mailflare:f4a17bd
echo "127.0.0.1 mailflare" | sudo tee -a /etc/hosts
```

`macro_services` is the network of the stack instance named `macro`; another
instance's is `<instance>_services`.

Why these flags:

- `HOST` equal to `APP_URL`'s host: Mailflare's Node build redirects
  `/.well-known/jmap` to the address it listens on, not to `APP_URL`, while
  the session uses `APP_URL`. The adapter refuses a redirect to another
  origin, so both must be `http://mailflare:3110`. `JmapOrigin` accepts
  plain HTTP to a single-label host such as `mailflare`.
- The `/etc/hosts` line lets the provisioning and seeding scripts on this
  machine use that origin too.
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

The workspace's HTTP client resolves names without `/etc/hosts`, so run the
test binary on the stack network, with the Nix store mounted as the stack's
services have it:

```sh
cargo test -p email_api_client --features outbound-jmap,test-support --lib --no-run
docker run --rm --network macro_services --user "$(id -u)" \
  -v /nix/store:/nix/store:ro -v <test binary>:/test-bin:ro \
  -v /path/to/credentials.json:/creds.json:ro -e JMAP_LIVE_CREDENTIALS=/creds.json \
  --entrypoint /test-bin macro-local-runtime:dev --ignored jmap::test::live
```

Set the credentials file's `origin` to `http://mailflare:3110`. The test
syncs every folder inside the read-only safety check and fails if the server
changed. Reset with `docker rm -f gmp-mailflare && docker volume rm
gmp-mailflare-data`.
