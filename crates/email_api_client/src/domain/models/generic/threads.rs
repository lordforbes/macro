//! Threads for mailboxes whose server does not group messages: every IMAP
//! mailbox, and JMAP servers that omit `threadId`.
//!
//! Messages are joined through their `Message-ID`, `In-Reply-To` and
//! `References` headers. Every id a message names joins its thread, including
//! ids of messages Macro has not seen, so two replies to a missing message
//! still share a thread.
//!
//! A thread is named by a hash of its root: the smallest member with no known
//! parent. A message that names an earlier one has a parent, and so does each
//! id after the first in a `References` chain. The root is normally the
//! original message (or the oldest id a reply names), so replies never rename
//! a thread, and the name depends only on the set of messages, not on the
//! order they arrive in. A thread is renamed only when a message links two
//! threads, or shows that a root had a parent after all; the ids that stop
//! naming a thread are reported so the caller can move their messages.
//!
//! The subject is a last resort: only a reply (`Re:`, `Fwd:`, ...) with no
//! linking headers at all joins other such replies by subject. A message with
//! no headers and no reply prefix is a thread of its own.

#[cfg(test)]
mod test;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use sha2::{Digest, Sha256};

/// A message's threading headers, as raw header values.
#[derive(Debug, Clone, Copy, Default)]
pub struct ThreadHeaders<'a> {
    /// `Message-ID`.
    pub message_id: Option<&'a str>,
    /// `In-Reply-To`.
    pub in_reply_to: Option<&'a str>,
    /// `References`.
    pub references: Option<&'a str>,
    /// `Subject`.
    pub subject: Option<&'a str>,
}

/// A thread id derived from headers. Stable for a given set of messages.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SyntheticThreadId(String);

impl SyntheticThreadId {
    /// The id as stored in `provider_thread_id`.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Where a newly indexed message went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadAssignment {
    /// The message's thread.
    pub thread_id: SyntheticThreadId,
    /// Thread ids this message retired, by joining their threads to this one
    /// or by revealing an earlier root. Their messages now belong to
    /// `thread_id` and must be moved there.
    pub merged: Vec<SyntheticThreadId>,
}

/// The id prefix, versioned so a change of scheme cannot collide with ids
/// already stored.
const THREAD_ID_PREFIX: &str = "syn1-";

/// Bytes of the hash kept in an id: 128 bits, ample for one mailbox.
const THREAD_ID_HASH_BYTES: usize = 16;

/// The most ids read from one `References` header. Long chains repeat the
/// same ancestors; the cap bounds the work a hostile header can cause.
const MAX_REFERENCES: usize = 100;

/// What a thread is built from. The derived order puts message ids first, so
/// a thread with any message id is named by one.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Node {
    /// A `Message-ID`, seen on a message or named by one.
    MessageId(String),
    /// A normalised subject, for replies with no linking headers.
    Subject(String),
    /// A message with nothing to link it, by its provider key.
    Message(String),
}

/// An in-memory index of messages and their threads.
#[derive(Debug, Default)]
pub struct SyntheticThreads {
    /// Union-find links between the nodes of one thread; a set's
    /// representative maps to itself. Unrelated to message parentage.
    union: HashMap<Node, Node>,
    /// Per representative, the thread's members with no known parent.
    roots: HashMap<Node, BTreeSet<Node>>,
    /// Per representative, the thread's smallest member, which names a thread
    /// whose every member has a parent (a reference loop).
    smallest: HashMap<Node, Node>,
    /// Nodes known to have a parent message.
    has_parent: HashSet<Node>,
    /// Each indexed message's own node, by the caller's message key.
    messages: BTreeMap<String, Node>,
}

impl SyntheticThreads {
    /// An empty index.
    pub fn new() -> Self {
        Self::default()
    }

    /// Index a message by the caller's key (its provider id) and report its
    /// thread. Indexing the same key again changes nothing.
    pub fn insert(&mut self, message_key: &str, headers: ThreadHeaders<'_>) -> ThreadAssignment {
        if let Some(node) = self.messages.get(message_key).cloned() {
            return ThreadAssignment {
                thread_id: self.thread_of(&node),
                merged: Vec::new(),
            };
        }

        let linked = nodes_for(message_key, headers);
        let nodes: Vec<Node> = std::iter::once(linked.own.clone())
            .chain(linked.named.iter().cloned())
            .collect();

        // The names of the threads this message touches, before it changes them.
        let mut names_before = Vec::new();
        for node in &nodes {
            if self.union.contains_key(node) {
                let representative = self.find(node);
                names_before.push(self.name(&representative));
            }
        }

        for node in &nodes {
            self.add(node);
        }
        for node in &linked.with_parent {
            self.mark_parented(node);
        }
        for node in &nodes {
            self.join(&linked.own, node);
        }
        self.messages
            .insert(message_key.to_owned(), linked.own.clone());

        let representative = self.find(&linked.own);
        let thread_id = thread_id_for(&self.name(&representative));
        let mut merged: Vec<SyntheticThreadId> = names_before
            .iter()
            .map(thread_id_for)
            .filter(|id| *id != thread_id)
            .collect();
        merged.sort();
        merged.dedup();
        ThreadAssignment { thread_id, merged }
    }

    /// The thread of an indexed message.
    pub fn thread_id(&self, message_key: &str) -> Option<SyntheticThreadId> {
        let node = self.messages.get(message_key)?;
        Some(self.thread_of(node))
    }

    fn thread_of(&self, node: &Node) -> SyntheticThreadId {
        thread_id_for(&self.name(&self.representative(node)))
    }

    /// The node a thread is named by: its smallest parentless member.
    fn name(&self, representative: &Node) -> Node {
        self.roots
            .get(representative)
            .and_then(|roots| roots.first())
            .or_else(|| self.smallest.get(representative))
            .unwrap_or(representative)
            .clone()
    }

    fn add(&mut self, node: &Node) {
        if self.union.contains_key(node) {
            return;
        }
        self.union.insert(node.clone(), node.clone());
        self.smallest.insert(node.clone(), node.clone());
        let mut roots = BTreeSet::new();
        if !self.has_parent.contains(node) {
            roots.insert(node.clone());
        }
        self.roots.insert(node.clone(), roots);
    }

    fn mark_parented(&mut self, node: &Node) {
        if !self.has_parent.insert(node.clone()) {
            return;
        }
        let representative = self.find(node);
        if let Some(roots) = self.roots.get_mut(&representative) {
            roots.remove(node);
        }
    }

    /// The representative of `node`'s set, without path compression.
    fn representative(&self, node: &Node) -> Node {
        let mut current = node;
        while let Some(next) = self.union.get(current) {
            if next == current {
                break;
            }
            current = next;
        }
        current.clone()
    }

    fn find(&mut self, node: &Node) -> Node {
        let representative = self.representative(node);
        // Path compression keeps later lookups short.
        let mut current = node.clone();
        while current != representative {
            let next = self
                .union
                .get(&current)
                .cloned()
                .unwrap_or_else(|| representative.clone());
            self.union.insert(current, representative.clone());
            current = next;
        }
        representative
    }

    /// Put two nodes in one thread, combining what each set knows.
    fn join(&mut self, left: &Node, right: &Node) {
        let keep = self.find(left);
        let absorb = self.find(right);
        if keep == absorb {
            return;
        }
        self.union.insert(absorb.clone(), keep.clone());
        let absorbed_roots = self.roots.remove(&absorb).unwrap_or_default();
        self.roots
            .entry(keep.clone())
            .or_default()
            .extend(absorbed_roots);
        if let Some(absorbed_smallest) = self.smallest.remove(&absorb) {
            let smallest = self
                .smallest
                .entry(keep)
                .or_insert_with(|| absorbed_smallest.clone());
            if absorbed_smallest < *smallest {
                *smallest = absorbed_smallest;
            }
        }
    }
}

/// What one message contributes to the index.
struct LinkedNodes {
    /// The message's own node.
    own: Node,
    /// Every other node it names.
    named: Vec<Node>,
    /// The nodes it shows to have a parent.
    with_parent: Vec<Node>,
}

/// The nodes a message joins. Its own node is its `Message-ID`; failing
/// that, the first id it names; failing that, its subject if it is a reply;
/// failing that, the message alone.
fn nodes_for(message_key: &str, headers: ThreadHeaders<'_>) -> LinkedNodes {
    let own_id = message_ids(headers.message_id).into_iter().next();
    let in_reply_to = message_ids(headers.in_reply_to);
    let references: Vec<String> = message_ids(headers.references)
        .into_iter()
        .take(MAX_REFERENCES)
        .collect();

    // Each id after the first in References follows the one before it, and a
    // message naming an earlier one is that one's reply.
    let mut with_parent: Vec<Node> = references
        .iter()
        .skip(1)
        .cloned()
        .map(Node::MessageId)
        .collect();
    let names_a_parent = !in_reply_to.is_empty() || !references.is_empty();

    let mut named: Vec<Node> = in_reply_to
        .into_iter()
        .chain(references)
        .map(Node::MessageId)
        .collect();
    named.sort();
    named.dedup();

    let own = match own_id {
        Some(id) => {
            let own = Node::MessageId(id);
            if names_a_parent {
                with_parent.push(own.clone());
            }
            named.retain(|node| *node != own);
            own
        }
        None if !named.is_empty() => named.remove(0),
        None => match headers.subject.and_then(reply_subject) {
            Some(subject) => Node::Subject(subject),
            None => Node::Message(message_key.to_owned()),
        },
    };
    LinkedNodes {
        own,
        named,
        with_parent,
    }
}

/// The message ids in a header value. A header that does not parse as a list
/// of `<id>`s is ignored rather than guessed at.
fn message_ids(header: Option<&str>) -> Vec<String> {
    let Some(header) = header else {
        return Vec::new();
    };
    match mailparse::msgidparse(header) {
        Ok(ids) => ids
            .iter()
            .map(|id| id.trim().to_owned())
            .filter(|id| !id.is_empty())
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// The base subject of a reply or forward, lower-cased; `None` when the
/// subject has no reply or forward prefix.
fn reply_subject(subject: &str) -> Option<String> {
    let prefixes = lazy_regex::regex!(r"(?i)^\s*(?:(?:re|fwd?|aw|sv|wg)\s*(?:\[\d+\])?\s*:\s*)+");
    let found = prefixes.find(subject)?;
    let base = subject[found.end()..].trim().to_lowercase();
    if base.is_empty() {
        return None;
    }
    Some(base)
}

fn thread_id_for(root: &Node) -> SyntheticThreadId {
    let tagged = match root {
        Node::MessageId(id) => format!("m:{id}"),
        Node::Subject(subject) => format!("s:{subject}"),
        Node::Message(key) => format!("k:{key}"),
    };
    let digest = Sha256::digest(tagged.as_bytes());
    let hex: String = digest
        .iter()
        .take(THREAD_ID_HASH_BYTES)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    SyntheticThreadId(format!("{THREAD_ID_PREFIX}{hex}"))
}
