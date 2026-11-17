import type { Accessor } from 'solid-js';
import { queryReadyGate } from '../gate';
import { useEmailLinksQuery } from './link';

export function useInboxEmail(
  linkId: Accessor<string | undefined>
): Accessor<string | undefined> {
  const links = useEmailLinksQuery();
  return () => {
    const id = linkId();
    if (!id || !queryReadyGate(links)) return undefined;
    return links.data.links.find((link) => link.id === id)?.email_address;
  };
}

/**
 * Whether the inbox is read-only in Macro: a generic (JMAP or IMAP) mailbox
 * that takes no replies, sends or organization changes. False while the
 * links load, so other inboxes never flash disabled; the server refuses a
 * write to a read-only inbox regardless.
 */
export function useInboxReadOnly(
  linkId: Accessor<string | undefined>
): Accessor<boolean> {
  const links = useEmailLinksQuery();
  return () => {
    const id = linkId();
    if (!id || !queryReadyGate(links)) return false;
    const link = links.data.links.find((candidate) => candidate.id === id);
    return link !== undefined && !link.capabilities.organize;
  };
}
