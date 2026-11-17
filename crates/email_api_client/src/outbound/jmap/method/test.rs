use std::collections::BTreeSet;

use strum::IntoEnumIterator;

use super::*;

/// The JMAP methods the read-only phases may send (brief, safety rule 1).
/// Blob downloads are the other allowed operation; they are not methods.
const ALLOWED: [&str; 7] = [
    "Core/echo",
    "Mailbox/get",
    "Mailbox/changes",
    "Email/query",
    "Email/get",
    "Email/changes",
    "Thread/get",
];

#[test]
fn the_adapter_can_only_express_allowlisted_methods() {
    let expressible: BTreeSet<&str> = ReadMethod::iter().map(ReadMethod::name).collect();
    let allowed: BTreeSet<&str> = ALLOWED.into_iter().collect();

    assert_eq!(expressible, allowed);
    assert!(expressible.iter().all(|name| !name.ends_with("/set")
        && !name.ends_with("/import")
        && !name.ends_with("/copy")));
}
