import type { EmailProvider } from './ProviderDialog';

/**
 * Whether an inbox's provider is one Macro connects through Google or
 * Microsoft sign-in. Generic JMAP and IMAP mailboxes are not: reconnect,
 * calendar-consent and add-inbox flows must never start for them, because
 * those flows open Google sign-in for any provider they don't recognize.
 */
export function isOAuthProvider(provider: string): provider is EmailProvider {
  return provider === 'GMAIL' || provider === 'OUTLOOK';
}

/** Narrows an inbox to one connected through Google or Microsoft sign-in. */
export function hasOAuthProvider<T extends { provider: string }>(
  link: T
): link is T & { provider: EmailProvider } {
  return isOAuthProvider(link.provider);
}
