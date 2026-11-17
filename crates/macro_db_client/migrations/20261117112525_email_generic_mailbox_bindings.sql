-- Additive: a new, empty table. Connection settings and the encrypted secret
-- of a generic (JMAP or IMAP) mailbox link. The authentication service writes
-- and decrypts it; the secret is an AES-256-GCM envelope whose data key is
-- encrypted by KMS with an encryption context naming the link.
SET LOCAL lock_timeout = '1s';

CREATE TABLE email_generic_mailbox_bindings (
    -- The row goes with its link. Deleting it does not revoke the secret at
    -- the mail server.
    link_id uuid PRIMARY KEY REFERENCES email_links(id) ON DELETE CASCADE,
    protocol text NOT NULL CHECK (protocol IN ('jmap', 'imap')),
    -- JMAP: the session origin, such as https://mail.example.com.
    endpoint text,
    -- IMAP: server, port and TLS mode. Plain-text connections are refused.
    host text,
    port integer CHECK (port BETWEEN 1 AND 65535),
    tls_mode text CHECK (tls_mode IN ('implicit', 'starttls')),
    username text NOT NULL CHECK (username <> ''),
    secret_ciphertext bytea NOT NULL CHECK (octet_length(secret_ciphertext) > 0),
    encrypted_data_key bytea NOT NULL CHECK (octet_length(encrypted_data_key) > 0),
    nonce bytea NOT NULL CHECK (octet_length(nonce) = 12),
    encryption_version smallint NOT NULL,
    kms_key_id text NOT NULL CHECK (kms_key_id <> ''),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CHECK (
        (protocol = 'jmap' AND endpoint IS NOT NULL
            AND host IS NULL AND port IS NULL AND tls_mode IS NULL)
        OR (protocol = 'imap' AND endpoint IS NULL
            AND host IS NOT NULL AND port IS NOT NULL AND tls_mode IS NOT NULL)
    )
);
