"""Deliver the generic-mailbox fixture set to the local Mailflare over SMTP.

Every address uses reserved example domains. Run against 127.0.0.1:2526 only.
"""

import os
import smtplib
from email.headerregistry import Address
from email.message import EmailMessage
from email.utils import format_datetime, make_msgid
from datetime import datetime, timedelta, timezone

HOST, PORT = "127.0.0.1", 2526
TO = Address("Reader", "reader", "example.com")
BASE = datetime(2026, 11, 1, 9, 0, tzinfo=timezone.utc)


def message(subject, sender, minutes, *, message_id=True, in_reply_to=None, references=None):
    msg = EmailMessage()
    msg["From"] = sender
    msg["To"] = TO
    msg["Subject"] = subject
    msg["Date"] = format_datetime(BASE + timedelta(minutes=minutes))
    if message_id is True:
        msg["Message-ID"] = make_msgid(domain="sender.example")
    elif message_id:
        msg["Message-ID"] = message_id
    if in_reply_to:
        msg["In-Reply-To"] = in_reply_to
    if references:
        msg["References"] = " ".join(references)
    return msg


alice = Address("Alice Example", "alice", "sender.example")
bob = Address("Bob Example", "bob", "sender.example")
fixtures = []

# Plain text.
plain = message("Plain text note", alice, 0)
plain.set_content("A plain text message.\n")
fixtures.append(plain)

# HTML only.
html = message("HTML only", bob, 5)
html.set_content("<p>An <b>HTML</b> message.</p>", subtype="html")
fixtures.append(html)

# multipart/alternative.
alternative = message("Text and HTML", alice, 10)
alternative.set_content("Plain version.\n")
alternative.add_alternative("<p>HTML version.</p>", subtype="html")
fixtures.append(alternative)

# Non-ASCII headers and body.
accents = message("Café crème, naïve résumé — 日本語", Address("Zoë Ünïcode", "zoe", "sender.example"), 15)
accents.set_content("Grüße aus Köln. Ça va? 你好。\n")
fixtures.append(accents)

# A large attachment (5 MiB) and a small one.
attached = message("Report with attachments", bob, 20)
attached.set_content("Two attachments.\n")
attached.add_attachment(os.urandom(5 * 1024 * 1024), maintype="application", subtype="octet-stream", filename="large.bin")
attached.add_attachment(b"col1,col2\n1,2\n", maintype="text", subtype="csv", filename="small.csv")
fixtures.append(attached)

# Missing Message-ID.
no_id = message("No message id here", alice, 25, message_id=None)
no_id.set_content("This message has no Message-ID.\n")
fixtures.append(no_id)

# Two different messages sharing one Message-ID.
shared_id = "<duplicate@sender.example>"
for index in (1, 2):
    duplicate = message(f"Duplicate id {index}", bob, 30 + index, message_id=shared_id)
    duplicate.set_content(f"Copy {index} of a reused Message-ID.\n")
    fixtures.append(duplicate)

# A thread with a long References chain, delivered newest first.
root_id = "<thread-root@sender.example>"
chain = [root_id]
thread = []
root = message("Planning the offsite", alice, 40, message_id=root_id)
root.set_content("Shall we plan the offsite?\n")
thread.append(root)
for step in range(1, 6):
    reply_id = f"<thread-reply-{step}@sender.example>"
    reply = message(
        "Re: Planning the offsite",
        bob if step % 2 else alice,
        40 + step,
        message_id=reply_id,
        in_reply_to=chain[-1],
        references=list(chain),
    )
    reply.set_content(f"Reply number {step}.\n")
    thread.append(reply)
    chain.append(reply_id)
fixtures.extend(reversed(thread))

with smtplib.SMTP(HOST, PORT, timeout=60) as smtp:
    for msg in fixtures:
        smtp.send_message(msg, from_addr=msg["From"].addresses[0].addr_spec, to_addrs=[TO.addr_spec])
print(f"delivered {len(fixtures)} messages")
