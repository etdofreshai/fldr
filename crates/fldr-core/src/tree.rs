//! Trees: content-addressed directory listings, and their flat form.
//!
//! A [`Tree`] is one directory level, mapping entry names to files or
//! subtrees. Its id is `Hash::of_typed(Tree, canonical_json(tree))`. On the
//! wire a tree is a plain JSON object:
//!
//! ```json
//! {"README.md":{"blob":"…","kind":"file","size":12,"type":"file"},
//!  "src":{"tree":"…","type":"dir"}}
//! ```
//!
//! A [`FlatTree`] maps every file's full [`RepoPath`] to its [`FileEntry`].
//! It is the convenient form for applying and diffing changes;
//! [`FlatTree::build_trees`] and [`FlatTree::from_trees`] convert between the
//! two. Directories are implicit: one exists exactly while it contains a
//! file, so empty directories are never stored. The only empty tree is the
//! root of an empty repository.

use std::collections::{BTreeMap, HashSet};
use std::ops::{Bound, Deref};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::canonical::{self, CanonicalError, MAX_SAFE_INTEGER};
use crate::hash::{BlobId, Hash, ObjectKind, TreeId};
use crate::ops::ApplyError;
use crate::path::{PathError, RepoPath, validate_component};

/// Largest representable file size: 2^53 − 1 bytes, the canonical JSON
/// integer limit.
pub const MAX_FILE_SIZE: u64 = MAX_SAFE_INTEGER;

/// How a file is materialized on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileKind {
    /// A regular file.
    File,
    /// A regular file with the executable bit set.
    Exec,
    /// A symbolic link; the blob holds the link target.
    Symlink,
}

/// A file: its contents, size in bytes and kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FileEntry {
    pub blob: BlobId,
    pub size: u64,
    pub kind: FileKind,
}

impl FileEntry {
    /// A regular file entry for `data`.
    pub fn of(data: &[u8], kind: FileKind) -> Self {
        Self {
            blob: BlobId::of(data),
            size: data.len() as u64,
            kind,
        }
    }
}

/// One entry of a [`Tree`]. Tagged with `"type"`: `"file"` or `"dir"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum TreeEntry {
    File(FileEntry),
    Dir { tree: TreeId },
}

/// One directory level, keyed by entry name.
///
/// Names are validated path components. The in-memory map is ordered by
/// bytes; the canonical encoding re-sorts by UTF-16 as JCS requires.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Tree {
    entries: BTreeMap<String, TreeEntry>,
}

impl Tree {
    /// The empty tree, the root of an empty repository.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds or replaces an entry. `name` must be a single valid component.
    pub fn insert(
        &mut self,
        name: impl Into<String>,
        entry: TreeEntry,
    ) -> Result<Option<TreeEntry>, PathError> {
        let name = name.into();
        validate_component(&name)?;
        Ok(self.entries.insert(name, entry))
    }

    pub fn get(&self, name: &str) -> Option<&TreeEntry> {
        self.entries.get(name)
    }

    pub fn entries(&self) -> &BTreeMap<String, TreeEntry> {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Canonical JSON encoding. Fails only if a file size exceeds
    /// [`MAX_FILE_SIZE`].
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, CanonicalError> {
        canonical::to_canonical(self)
    }

    /// Decodes a tree, rejecting non-canonical bytes and invalid names.
    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, CanonicalError> {
        canonical::from_canonical(bytes)
    }

    /// `Hash::of_typed(ObjectKind::Tree, canonical_bytes)`.
    pub fn id(&self) -> Result<TreeId, CanonicalError> {
        Ok(Self::id_of_bytes(&self.canonical_bytes()?))
    }

    /// The id of an already-encoded tree.
    pub fn id_of_bytes(canonical: &[u8]) -> TreeId {
        TreeId(Hash::of_typed(ObjectKind::Tree, canonical))
    }

    /// The id of the empty tree, `sha256("tree\0{}")`.
    pub fn empty_id() -> TreeId {
        Self::id_of_bytes(b"{}")
    }
}

impl Serialize for Tree {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.entries.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Tree {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let entries = BTreeMap::<String, TreeEntry>::deserialize(deserializer)?;
        for name in entries.keys() {
            validate_component(name)
                .map_err(|e| serde::de::Error::custom(format!("tree entry {name:?}: {e}")))?;
        }
        Ok(Self { entries })
    }
}

/// Errors from [`FlatTree::from_trees`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TreeError {
    #[error("tree {0} not found")]
    Missing(TreeId),
    #[error("tree looked up as {expected} hashes to {actual}")]
    IdMismatch { expected: TreeId, actual: TreeId },
    #[error("directory {0} is empty; empty directories are never stored")]
    EmptyDir(RepoPath),
    #[error("invalid path under tree: {0}")]
    Path(#[from] PathError),
    #[error(transparent)]
    Canonical(#[from] CanonicalError),
    #[error(transparent)]
    Apply(#[from] ApplyError),
}

/// Every file in a repository snapshot, keyed by full path.
///
/// Always well formed: no file lies "under" another file, and every size is
/// at most [`MAX_FILE_SIZE`]. Construct it with [`FlatTree::insert`],
/// [`FlatTree::from_files`], [`FlatTree::apply`] or by deserializing (a JSON
/// object from path to [`FileEntry`]). Read access goes through `Deref` to
/// the underlying `BTreeMap`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct FlatTree {
    files: BTreeMap<RepoPath, FileEntry>,
}

impl Deref for FlatTree {
    type Target = BTreeMap<RepoPath, FileEntry>;

    fn deref(&self) -> &Self::Target {
        &self.files
    }
}

impl<'de> Deserialize<'de> for FlatTree {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let files = BTreeMap::<RepoPath, FileEntry>::deserialize(deserializer)?;
        FlatTree::from_files(files).map_err(serde::de::Error::custom)
    }
}

impl FlatTree {
    pub fn new() -> Self {
        Self::default()
    }

    /// Builds a flat tree from `(path, entry)` pairs, rejecting conflicts.
    pub fn from_files(
        files: impl IntoIterator<Item = (RepoPath, FileEntry)>,
    ) -> Result<Self, ApplyError> {
        let mut flat = FlatTree::new();
        for (path, entry) in files {
            flat.insert(path, entry)?;
        }
        Ok(flat)
    }

    pub fn into_map(self) -> BTreeMap<RepoPath, FileEntry> {
        self.files
    }

    /// Adds or replaces the file at `path`. Fails if `path` lies under an
    /// existing file ([`ApplyError::UnderFile`]), if files exist under
    /// `path` ([`ApplyError::IsDirectory`]), or if the size is too large.
    pub fn insert(
        &mut self,
        path: RepoPath,
        entry: FileEntry,
    ) -> Result<Option<FileEntry>, ApplyError> {
        if entry.size > MAX_FILE_SIZE {
            return Err(ApplyError::TooLarge {
                path,
                size: entry.size,
            });
        }
        let under = path
            .ancestors()
            .find_map(|a| self.files.get_key_value(a))
            .map(|(file, _)| file.clone());
        if let Some(file) = under {
            return Err(ApplyError::UnderFile { file, path });
        }
        if self.has_descendants(path.as_str()) {
            return Err(ApplyError::IsDirectory(path));
        }
        Ok(self.files.insert(path, entry))
    }

    /// Removes and returns the file at `path`, if any.
    pub fn remove(&mut self, path: &str) -> Option<FileEntry> {
        self.files.remove(path)
    }

    /// True if any file lies strictly under directory `dir`.
    pub fn has_descendants(&self, dir: &str) -> bool {
        let prefix = format!("{dir}/");
        // Strings sharing a prefix are contiguous in byte order.
        self.files
            .range::<str, _>((Bound::Included(prefix.as_str()), Bound::Unbounded))
            .next()
            .is_some_and(|(p, _)| p.as_str().starts_with(&prefix))
    }

    /// Builds the [`Tree`] objects for this snapshot.
    ///
    /// Returns the root id and every distinct tree, children before parents
    /// (so a store can insert them in order), the root last. The order is a
    /// post-order walk by entry name and depends only on the contents. An
    /// empty `FlatTree` yields the empty tree ([`Tree::empty_id`]).
    pub fn build_trees(&self) -> (TreeId, Vec<(TreeId, Tree)>) {
        enum Node<'a> {
            File(FileEntry),
            Dir(BTreeMap<&'a str, Node<'a>>),
        }

        let mut root: BTreeMap<&str, Node> = BTreeMap::new();
        for (path, entry) in &self.files {
            let mut dir = &mut root;
            let mut parts = path.components().peekable();
            while let Some(name) = parts.next() {
                if parts.peek().is_none() {
                    dir.insert(name, Node::File(*entry));
                } else {
                    let node = dir
                        .entry(name)
                        .or_insert_with(|| Node::Dir(BTreeMap::new()));
                    dir = match node {
                        Node::Dir(children) => children,
                        // Excluded by the FlatTree invariant.
                        Node::File(_) => unreachable!("file {path} lies under a file"),
                    };
                }
            }
        }

        fn emit(
            dir: &BTreeMap<&str, Node>,
            seen: &mut HashSet<TreeId>,
            out: &mut Vec<(TreeId, Tree)>,
        ) -> TreeId {
            let mut tree = Tree::new();
            for (name, node) in dir {
                let entry = match node {
                    Node::File(f) => TreeEntry::File(*f),
                    Node::Dir(children) => TreeEntry::Dir {
                        tree: emit(children, seen, out),
                    },
                };
                // Names came from a validated RepoPath, so this is a plain insert.
                tree.entries.insert((*name).to_owned(), entry);
            }
            let id = tree
                .id()
                .expect("FlatTree sizes are within MAX_FILE_SIZE, so trees always encode");
            if seen.insert(id) {
                out.push((id, tree));
            }
            id
        }

        let mut out = Vec::new();
        let root_id = emit(&root, &mut HashSet::new(), &mut out);
        (root_id, out)
    }

    /// Flattens the tree rooted at `root`, fetching each tree with `lookup`.
    ///
    /// The inverse of [`build_trees`](Self::build_trees). Every fetched tree
    /// is re-hashed and must match the id it was looked up by; a non-root
    /// empty tree is rejected because empty directories are never stored.
    pub fn from_trees(
        root: TreeId,
        mut lookup: impl FnMut(&TreeId) -> Option<Tree>,
    ) -> Result<Self, TreeError> {
        let mut flat = FlatTree::new();
        let mut stack: Vec<(Option<RepoPath>, TreeId)> = vec![(None, root)];
        while let Some((dir, id)) = stack.pop() {
            let tree = lookup(&id).ok_or(TreeError::Missing(id))?;
            let actual = tree.id()?;
            if actual != id {
                return Err(TreeError::IdMismatch {
                    expected: id,
                    actual,
                });
            }
            if let (Some(dir), true) = (&dir, tree.is_empty()) {
                return Err(TreeError::EmptyDir(dir.clone()));
            }
            for (name, entry) in tree.entries {
                let path = match &dir {
                    Some(d) => d.join(&name)?,
                    None => RepoPath::new(name)?,
                };
                match entry {
                    TreeEntry::File(f) => {
                        flat.insert(path, f)?;
                    }
                    TreeEntry::Dir { tree } => stack.push((Some(path), tree)),
                }
            }
        }
        Ok(flat)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::collections::HashMap;

    pub(crate) fn p(s: &str) -> RepoPath {
        RepoPath::new(s).unwrap()
    }

    pub(crate) fn file(data: &str) -> FileEntry {
        FileEntry::of(data.as_bytes(), FileKind::File)
    }

    /// Random well-formed flat trees over a small alphabet, so paths collide
    /// and nest often.
    pub(crate) fn arb_flat() -> impl Strategy<Value = FlatTree> {
        let path = prop::collection::vec(
            prop::sample::select(vec!["a", "b", "c.txt", "é", "😀"]),
            1..4,
        )
        .prop_map(|parts| p(&parts.join("/")));
        let entry = (
            any::<u8>(),
            prop::sample::select(vec![FileKind::File, FileKind::Exec, FileKind::Symlink]),
        )
            .prop_map(|(b, kind)| FileEntry::of(&[b], kind));
        prop::collection::vec((path, entry), 0..12).prop_map(|files| {
            let mut flat = FlatTree::new();
            for (path, entry) in files {
                let _ = flat.insert(path, entry);
            }
            flat
        })
    }

    #[test]
    fn wire_format() {
        let mut t = Tree::new();
        t.insert("x", TreeEntry::File(file("hi"))).unwrap();
        t.insert(
            "d",
            TreeEntry::Dir {
                tree: Tree::empty_id(),
            },
        )
        .unwrap();
        let json = String::from_utf8(t.canonical_bytes().unwrap()).unwrap();
        assert_eq!(
            json,
            format!(
                r#"{{"d":{{"tree":"{}","type":"dir"}},"x":{{"blob":"{}","kind":"file","size":2,"type":"file"}}}}"#,
                Tree::empty_id(),
                BlobId::of(b"hi")
            )
        );
        assert_eq!(Tree::from_canonical_bytes(json.as_bytes()).unwrap(), t);
    }

    #[test]
    fn tree_names_validated() {
        assert!(
            Tree::new()
                .insert("a/b", TreeEntry::File(file("")))
                .is_err()
        );
        assert!(Tree::new().insert("..", TreeEntry::File(file(""))).is_err());
        let blob = BlobId::of(b"");
        let bad = format!(r#"{{"a/b":{{"blob":"{blob}","kind":"file","size":0,"type":"file"}}}}"#);
        assert!(Tree::from_canonical_bytes(bad.as_bytes()).is_err());
        let bad_kind =
            format!(r#"{{"a":{{"blob":"{blob}","kind":"File","size":0,"type":"file"}}}}"#);
        assert!(Tree::from_canonical_bytes(bad_kind.as_bytes()).is_err());
    }

    #[test]
    fn empty_tree() {
        assert_eq!(Tree::new().id().unwrap(), Tree::empty_id());
        let (root, trees) = FlatTree::new().build_trees();
        assert_eq!(root, Tree::empty_id());
        assert_eq!(trees, vec![(root, Tree::new())]);
        let map: HashMap<_, _> = trees.into_iter().collect();
        assert_eq!(
            FlatTree::from_trees(root, |id| map.get(id).cloned()).unwrap(),
            FlatTree::new()
        );
    }

    #[test]
    fn insert_rejects_conflicts() {
        let mut flat = FlatTree::new();
        flat.insert(p("a/b"), file("1")).unwrap();
        assert_eq!(
            flat.insert(p("a"), file("2")),
            Err(ApplyError::IsDirectory(p("a")))
        );
        assert_eq!(
            flat.insert(p("a/b/c"), file("2")),
            Err(ApplyError::UnderFile {
                path: p("a/b/c"),
                file: p("a/b")
            })
        );
        // Prefix-but-not-ancestor names are fine.
        flat.insert(p("a.txt"), file("3")).unwrap();
        flat.insert(p("ab"), file("3")).unwrap();
        flat.insert(p("a/bc"), file("3")).unwrap();
        let huge = FileEntry {
            size: MAX_FILE_SIZE + 1,
            ..file("")
        };
        assert!(matches!(
            flat.insert(p("z"), huge),
            Err(ApplyError::TooLarge { .. })
        ));
    }

    #[test]
    fn build_trees_shape() {
        let flat = FlatTree::from_files([
            (p("README"), file("r")),
            (p("src/a.rs"), file("a")),
            (p("src/x/y.rs"), file("y")),
            (p("docs/x/y.rs"), file("y")), // identical subtree to src/x
        ])
        .unwrap();
        let (root, trees) = flat.build_trees();
        assert_eq!(trees.last().unwrap().0, root);
        // x (deduplicated), docs, src, root
        assert_eq!(trees.len(), 4);
        let root_tree = &trees.last().unwrap().1;
        assert_eq!(
            root_tree.entries().keys().collect::<Vec<_>>(),
            ["README", "docs", "src"]
        );
        // Every child appears before its parent.
        let mut seen = HashSet::new();
        for (id, tree) in &trees {
            assert_eq!(tree.id().unwrap(), *id);
            for e in tree.entries().values() {
                if let TreeEntry::Dir { tree } = e {
                    assert!(seen.contains(tree));
                }
            }
            seen.insert(*id);
        }
    }

    #[test]
    fn from_trees_checks_integrity() {
        let flat = FlatTree::from_files([(p("d/f"), file("x"))]).unwrap();
        let (root, trees) = flat.build_trees();
        let map: HashMap<_, _> = trees.iter().cloned().collect();
        let sub = trees[0].0;

        let err = FlatTree::from_trees(root, |id| (id != &sub).then(|| map[id].clone()));
        assert_eq!(err, Err(TreeError::Missing(sub)));

        let err = FlatTree::from_trees(root, |id| {
            Some(if id == &sub {
                Tree::new()
            } else {
                map[id].clone()
            })
        });
        assert!(matches!(err, Err(TreeError::IdMismatch { .. })));

        let mut with_empty = Tree::new();
        with_empty
            .insert(
                "e",
                TreeEntry::Dir {
                    tree: Tree::empty_id(),
                },
            )
            .unwrap();
        let wid = with_empty.id().unwrap();
        let err = FlatTree::from_trees(wid, |id| {
            Some(if *id == wid {
                with_empty.clone()
            } else {
                Tree::new()
            })
        });
        assert_eq!(err, Err(TreeError::EmptyDir(p("e"))));
    }

    #[test]
    fn flat_tree_serde_validates() {
        let blob = BlobId::of(b"");
        let e = format!(r#"{{"blob":"{blob}","kind":"file","size":0}}"#);
        let ok = format!(r#"{{"a/b":{e},"a/c":{e}}}"#);
        assert_eq!(serde_json::from_str::<FlatTree>(&ok).unwrap().len(), 2);
        let bad = format!(r#"{{"a":{e},"a/c":{e}}}"#);
        assert!(serde_json::from_str::<FlatTree>(&bad).is_err());
    }

    proptest! {
        #[test]
        fn build_trees_round_trips(flat in arb_flat()) {
            let (root, trees) = flat.build_trees();
            prop_assert_eq!(trees.last().unwrap().0, root);
            let map: HashMap<_, _> = trees.into_iter().collect();
            let back = FlatTree::from_trees(root, |id| map.get(id).cloned()).unwrap();
            prop_assert_eq!(back, flat);
        }

        #[test]
        fn build_trees_ignores_insertion_order(flat in arb_flat(), seed in any::<u64>()) {
            let mut files: Vec<_> = flat.iter().map(|(p, e)| (p.clone(), *e)).collect();
            // Deterministic shuffle from the seed.
            let mut s = seed | 1;
            for i in (1..files.len()).rev() {
                s ^= s << 13; s ^= s >> 7; s ^= s << 17;
                files.swap(i, (s % (i as u64 + 1)) as usize);
            }
            let shuffled = FlatTree::from_files(files).unwrap();
            prop_assert_eq!(shuffled.build_trees(), flat.build_trees());
        }

        #[test]
        fn tree_canonical_bytes_stable(flat in arb_flat()) {
            for (id, tree) in flat.build_trees().1 {
                let bytes = tree.canonical_bytes().unwrap();
                prop_assert_eq!(Tree::id_of_bytes(&bytes), id);
                let back = Tree::from_canonical_bytes(&bytes).unwrap();
                prop_assert_eq!(back.canonical_bytes().unwrap(), bytes);
            }
        }
    }
}
