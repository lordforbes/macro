-- Additive: generic JMAP and IMAP mailboxes. No existing row changes, and no
-- deployed code writes these values. Enum values must be committed before use,
-- so nothing else goes in this migration.
SET LOCAL lock_timeout = '1s';

ALTER TYPE email_user_provider_enum ADD VALUE IF NOT EXISTS 'JMAP';
ALTER TYPE email_user_provider_enum ADD VALUE IF NOT EXISTS 'IMAP';
