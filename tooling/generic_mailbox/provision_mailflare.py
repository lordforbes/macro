"""Provision a local Mailflare test server: an admin account and a JMAP key.

Usage: python3 -I provision_mailflare.py <credentials-out.json>

Test-only credentials for a disposable local server. The output file is
created with mode 0600 and the password is written before anything else, so
the account is never left without a known password.
"""

import json
import os
import secrets
import sys
import urllib.error
import urllib.request

BASE = "http://127.0.0.1:3110"
ORIGIN = "http://localhost:3110"
ADDRESS = "reader@example.com"
COOKIES = {}


def call(method, path, body=None):
    data = None if body is None else json.dumps(body).encode()
    request = urllib.request.Request(BASE + path, data=data, method=method)
    request.add_header("Content-Type", "application/json")
    request.add_header("Sec-Fetch-Site", "same-origin")
    if COOKIES:
        request.add_header("Cookie", "; ".join(f"{k}={v}" for k, v in COOKIES.items()))
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            # The production build marks cookies Secure; send them back by
            # hand over this local plain-HTTP connection.
            for header in response.headers.get_all("Set-Cookie") or []:
                name, _, rest = header.partition("=")
                COOKIES[name.strip()] = rest.split(";", 1)[0]
            return response.status, json.loads(response.read() or b"null")
    except urllib.error.HTTPError as error:
        return error.code, error.read().decode()[:300]


def save(path, values):
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w") as out:
        json.dump(values, out)


def main(path):
    password = secrets.token_urlsafe(18)
    save(path, {"origin": ORIGIN, "address": ADDRESS, "password": password})

    status, reply = call("POST", "/api/auth/register", {
        "domain": "example.com",
        "username": "reader",
        "password": password,
        "resetEmail": "reset@example.com",
        "skipDns": True,
    })
    if status != 200:
        sys.exit(f"register failed: {status} {reply}")

    # Registration does not open a session; signing in does.
    status, reply = call("POST", "/api/auth/login", {"email": ADDRESS, "password": password})
    if status != 200:
        sys.exit(f"login failed: {status} {reply}")

    status, mailboxes = call("GET", "/api/mailboxes")
    if status != 200:
        sys.exit(f"mailbox list failed: {status} {mailboxes}")
    mailbox_ids = [mailbox["id"] for mailbox in mailboxes["mailboxes"]]

    # An app-password key: the jmap scope only, limited to these mailboxes.
    status, key = call("POST", "/api/api-keys", {
        "name": "macro-generic-mailbox-test",
        "scopes": ["jmap"],
        "mailboxIds": mailbox_ids,
    })
    if status != 200:
        sys.exit(f"api key failed: {status} {key}")

    save(path, {
        "origin": ORIGIN,
        "address": ADDRESS,
        "password": password,
        "jmap_key": key["key"],
        "mailbox_ids": mailbox_ids,
    })
    print(f"provisioned {ADDRESS}; credentials in {path}")


if __name__ == "__main__":
    main(sys.argv[1])
