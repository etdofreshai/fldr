//! Changesets: immutable, content-addressed units of history.
//!
//! A changeset lists only the paths that changed relative to its first
//! parent (p1), "Bonsai" style. Its id is
//! `Hash::of_typed(Changeset, canonical_json(cs))`. On the wire:
//!
//! ```json
//! {"actor":{"kind":"human","name":"et"},
//!  "changes":{"a.txt":{"blob":"…","kind":"file","op":"put","size":3},"old":{"op":"delete"}},
//!  "message":"why","parents":["…"],"time":1760659200}
//! ```
//!
//! Landing time and the actor who moved a ref live in the server's
//! `ref_log`, not here.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::canonical::{self, CanonicalError};
use crate::hash::{BlobId, ChangesetId, Hash, ObjectKind};
use crate::path::RepoPath;
use crate::tree::{FileEntry, FileKind};

/// Maximum commit message length, in bytes (64 KiB).
pub const MAX_MESSAGE_LEN: usize = 64 * 1024;

/// Maximum number of parents in M0. Octopus merges (3+ parents) are not
/// supported yet; the wire format allows them, so lifting this limit later
/// does not change any existing changeset id.
pub const MAX_PARENTS: usize = 2;

/// What kind of principal authored a changeset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActorKind {
    Human,
    Agent,
    Service,
}

/// The author of a changeset.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Actor {
    pub kind: ActorKind,
    /// Display or account name.
    pub name: String,
    /// For agents: which agent, e.g. `"claude-code"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// For agents: the session that produced the change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
}

impl Actor {
    pub fn new(kind: ActorKind, name: impl Into<String>) -> Self {
        Self {
            kind,
            name: name.into(),
            agent: None,
            session: None,
        }
    }
}

/// A change to one path. Tagged with `"op"`: `"put"` or `"delete"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum FileChange {
    /// Create or replace the file.
    Put {
        blob: BlobId,
        size: u64,
        kind: FileKind,
    },
    /// Remove the file.
    Delete,
}

impl From<FileEntry> for FileChange {
    fn from(e: FileEntry) -> Self {
        FileChange::Put {
            blob: e.blob,
            size: e.size,
            kind: e.kind,
        }
    }
}

impl FileChange {
    /// The resulting file, or `None` for a delete.
    pub fn entry(&self) -> Option<FileEntry> {
        match *self {
            FileChange::Put { blob, size, kind } => Some(FileEntry { blob, size, kind }),
            FileChange::Delete => None,
        }
    }
}

/// A set of path changes, keyed and sorted by path.
pub type Changes = BTreeMap<RepoPath, FileChange>;

/// An immutable changeset.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Changeset {
    /// p1 first. Empty for a root changeset; two for a merge, whose
    /// `changes` are relative to p1 only.
    pub parents: Vec<ChangesetId>,
    /// Changes relative to p1 (or to the empty tree for a root).
    pub changes: Changes,
    pub actor: Actor,
    /// Why the change was made. At most [`MAX_MESSAGE_LEN`] bytes.
    pub message: String,
    /// Author time, Unix seconds UTC, as supplied by the client.
    pub time: i64,
}

/// Why a changeset is invalid.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ChangesetError {
    #[error("parent {0} is listed more than once")]
    DuplicateParent(ChangesetId),
    #[error("{0} parents; at most {MAX_PARENTS} are supported (no octopus merges yet)")]
    TooManyParents(usize),
    #[error("message is {0} bytes, max {MAX_MESSAGE_LEN}")]
    MessageTooLong(usize),
    #[error("a non-merge changeset with a parent must change at least one path")]
    Empty,
    #[error("changeset does not round-trip through canonical JSON: {0}")]
    Canonical(#[from] CanonicalError),
}

impl Changeset {
    /// p1, or `None` for a root changeset.
    pub fn p1(&self) -> Option<&ChangesetId> {
        self.parents.first()
    }

    pub fn is_root(&self) -> bool {
        self.parents.is_empty()
    }

    pub fn is_merge(&self) -> bool {
        self.parents.len() > 1
    }

    /// Canonical JSON encoding. Fails if a size or the time is outside
    /// ±(2^53−1).
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, CanonicalError> {
        canonical::to_canonical(self)
    }

    /// Decodes a changeset, rejecting non-canonical bytes. Does not run
    /// [`validate`](Self::validate).
    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, CanonicalError> {
        canonical::from_canonical(bytes)
    }

    /// `Hash::of_typed(ObjectKind::Changeset, canonical_bytes)`.
    pub fn id(&self) -> Result<ChangesetId, CanonicalError> {
        Ok(Self::id_of_bytes(&self.canonical_bytes()?))
    }

    /// The id of an already-encoded changeset.
    pub fn id_of_bytes(canonical: &[u8]) -> ChangesetId {
        ChangesetId(Hash::of_typed(ObjectKind::Changeset, canonical))
    }

    /// Checks the structural rules that do not need history:
    /// - parents are distinct, and there are at most [`MAX_PARENTS`];
    /// - the message is at most [`MAX_MESSAGE_LEN`] bytes;
    /// - a changeset with exactly one parent changes at least one path (a
    ///   root may be empty, to start a repository; a merge may be empty,
    ///   when p1 already has the merged content);
    /// - it encodes canonically and decodes back to itself.
    ///
    /// Whether the changes apply to p1's tree is checked by the server with
    /// [`FlatTree::apply`](crate::FlatTree::apply).
    pub fn validate(&self) -> Result<(), ChangesetError> {
        let mut seen = HashSet::new();
        for p in &self.parents {
            if !seen.insert(p) {
                return Err(ChangesetError::DuplicateParent(*p));
            }
        }
        if self.parents.len() > MAX_PARENTS {
            return Err(ChangesetError::TooManyParents(self.parents.len()));
        }
        if self.message.len() > MAX_MESSAGE_LEN {
            return Err(ChangesetError::MessageTooLong(self.message.len()));
        }
        if self.parents.len() == 1 && self.changes.is_empty() {
            return Err(ChangesetError::Empty);
        }
        let bytes = self.canonical_bytes()?;
        if Self::from_canonical_bytes(&bytes)? != *self {
            return Err(CanonicalError::NotCanonical.into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canonical::MAX_SAFE_INTEGER;

    fn cs(parents: Vec<ChangesetId>, changes: Changes) -> Changeset {
        Changeset {
            parents,
            changes,
            actor: Actor::new(ActorKind::Human, "et"),
            message: "m".into(),
            time: 0,
        }
    }

    fn pid(n: u8) -> ChangesetId {
        ChangesetId(Hash::of(&[n]))
    }

    fn one_change() -> Changes {
        BTreeMap::from([(RepoPath::new("a").unwrap(), FileChange::Delete)])
    }

    #[test]
    fn wire_format() {
        let blob = BlobId::of(b"abc");
        let mut c = cs(vec![pid(1)], one_change());
        c.changes.insert(
            RepoPath::new("b").unwrap(),
            FileChange::Put {
                blob,
                size: 3,
                kind: FileKind::Exec,
            },
        );
        c.actor = Actor {
            kind: ActorKind::Agent,
            name: "bot".into(),
            agent: Some("claude-code".into()),
            session: Some("s1".into()),
        };
        c.time = -5;
        let json = String::from_utf8(c.canonical_bytes().unwrap()).unwrap();
        assert_eq!(
            json,
            format!(
                concat!(
                    r#"{{"actor":{{"agent":"claude-code","kind":"agent","name":"bot","session":"s1"}},"#,
                    r#""changes":{{"a":{{"op":"delete"}},"b":{{"blob":"{}","kind":"exec","op":"put","size":3}}}},"#,
                    r#""message":"m","parents":["{}"],"time":-5}}"#
                ),
                blob,
                pid(1)
            )
        );
        assert_eq!(Changeset::from_canonical_bytes(json.as_bytes()).unwrap(), c);
        assert_eq!(c.id().unwrap(), Changeset::id_of_bytes(json.as_bytes()));
    }

    #[test]
    fn optional_actor_fields_omitted() {
        let json = serde_json::to_string(&Actor::new(ActorKind::Service, "ci")).unwrap();
        assert_eq!(json, r#"{"kind":"service","name":"ci"}"#);
    }

    #[test]
    fn validation() {
        assert_eq!(cs(vec![], Changes::new()).validate(), Ok(()));
        assert_eq!(cs(vec![pid(1)], one_change()).validate(), Ok(()));
        assert_eq!(cs(vec![pid(1), pid(2)], Changes::new()).validate(), Ok(()));
        assert_eq!(
            cs(vec![pid(1)], Changes::new()).validate(),
            Err(ChangesetError::Empty)
        );
        assert_eq!(
            cs(vec![pid(1), pid(1)], one_change()).validate(),
            Err(ChangesetError::DuplicateParent(pid(1)))
        );
        assert_eq!(
            cs(vec![pid(1), pid(2), pid(3)], one_change()).validate(),
            Err(ChangesetError::TooManyParents(3))
        );
        let mut c = cs(vec![], one_change());
        c.message = "x".repeat(MAX_MESSAGE_LEN);
        assert_eq!(c.validate(), Ok(()));
        c.message.push('x');
        assert_eq!(
            c.validate(),
            Err(ChangesetError::MessageTooLong(MAX_MESSAGE_LEN + 1))
        );
        let mut c = cs(vec![], one_change());
        c.time = MAX_SAFE_INTEGER as i64 + 1;
        assert!(matches!(c.validate(), Err(ChangesetError::Canonical(_))));
    }

    #[test]
    fn rejects_unknown_fields_and_bad_tags() {
        let ok = r#"{"actor":{"kind":"human","name":"et"},"changes":{},"message":"","parents":[],"time":0}"#;
        assert!(Changeset::from_canonical_bytes(ok.as_bytes()).is_ok());
        // Unknown fields are dropped on decode, so the re-encoding differs.
        let extra = r#"{"actor":{"kind":"human","name":"et"},"changes":{},"extra":1,"message":"","parents":[],"time":0}"#;
        assert!(Changeset::from_canonical_bytes(extra.as_bytes()).is_err());
        let bad_kind = r#"{"actor":{"kind":"Human","name":"et"},"changes":{},"message":"","parents":[],"time":0}"#;
        assert!(Changeset::from_canonical_bytes(bad_kind.as_bytes()).is_err());
        let bad_path = r#"{"actor":{"kind":"human","name":"et"},"changes":{"../x":{"op":"delete"}},"message":"","parents":[],"time":0}"#;
        assert!(Changeset::from_canonical_bytes(bad_path.as_bytes()).is_err());
    }
}
