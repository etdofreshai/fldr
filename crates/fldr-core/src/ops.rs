//! Pure operations on snapshots and change sets: apply, diff and overlap.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Bound;

use crate::changeset::{Changes, FileChange};
use crate::path::RepoPath;
use crate::tree::FlatTree;

/// Why a set of changes cannot be applied to a [`FlatTree`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ApplyError {
    #[error("cannot delete {0}: no such file")]
    DeleteMissing(RepoPath),
    #[error("cannot put {0}: it is a directory")]
    IsDirectory(RepoPath),
    #[error("cannot put {path}: {file} is a file")]
    UnderFile { path: RepoPath, file: RepoPath },
    #[error("{path} is {size} bytes, larger than the maximum")]
    TooLarge { path: RepoPath, size: u64 },
}

impl FlatTree {
    /// Applies `changes` and returns the new snapshot.
    ///
    /// All deletes are applied before any put, so one change set can replace
    /// a file with a directory of the same name (delete `a`, put `a/b`) or
    /// the reverse. Fails on deleting a missing path, putting a file where a
    /// directory remains, or putting a file under a remaining file.
    pub fn apply(&self, changes: &Changes) -> Result<FlatTree, ApplyError> {
        let mut out = self.clone();
        for (path, change) in changes {
            if matches!(change, FileChange::Delete) && out.remove(path.as_str()).is_none() {
                return Err(ApplyError::DeleteMissing(path.clone()));
            }
        }
        for (path, change) in changes {
            if let Some(entry) = change.entry() {
                out.insert(path.clone(), entry)?;
            }
        }
        Ok(out)
    }
}

/// The changes that turn `a` into `b`: `a.apply(&diff(a, b)) == Ok(b)`.
pub fn diff(a: &FlatTree, b: &FlatTree) -> Changes {
    let mut changes = BTreeMap::new();
    for (path, entry) in a.iter() {
        if !b.contains_key(path) {
            changes.insert(path.clone(), FileChange::Delete);
        } else if b[path] != *entry {
            changes.insert(path.clone(), b[path].into());
        }
    }
    for (path, entry) in b.iter() {
        if !a.contains_key(path) {
            changes.insert(path.clone(), (*entry).into());
        }
    }
    changes
}

/// The paths touched by `changes`.
pub fn changed_paths(changes: &Changes) -> BTreeSet<RepoPath> {
    changes.keys().cloned().collect()
}

/// True if two path sets conflict: they share a path, or a path in one is an
/// ancestor directory of a path in the other. `land` rejects a rebase when
/// the paths changed since the base overlap the changeset's own paths.
pub fn paths_overlap(a: &BTreeSet<RepoPath>, b: &BTreeSet<RepoPath>) -> bool {
    a.iter().any(|x| !conflicts_with(x, b).is_empty())
}

/// Every path, from either side, involved in an overlap between `a` and `b`
/// (see [`paths_overlap`]), in byte order. Empty iff they do not overlap.
/// Suitable for listing in a 409 response.
pub fn overlapping_paths(a: &BTreeSet<RepoPath>, b: &BTreeSet<RepoPath>) -> BTreeSet<RepoPath> {
    let mut out = BTreeSet::new();
    for x in a {
        let hits = conflicts_with(x, b);
        if !hits.is_empty() {
            out.insert(x.clone());
            out.extend(hits.into_iter().cloned());
        }
    }
    out
}

/// Paths in `set` equal to `x`, ancestors of `x`, or descendants of `x`.
fn conflicts_with<'a>(x: &RepoPath, set: &'a BTreeSet<RepoPath>) -> Vec<&'a RepoPath> {
    let mut hits: Vec<&RepoPath> = x
        .ancestors()
        .chain(std::iter::once(x.as_str()))
        .filter_map(|s| set.get(s))
        .collect();
    // Descendants of x share the prefix "x/" and are contiguous in byte order.
    let prefix = format!("{x}/");
    hits.extend(
        set.range::<str, _>((Bound::Included(prefix.as_str()), Bound::Unbounded))
            .take_while(|p| p.as_str().starts_with(&prefix)),
    );
    hits
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::tests::{arb_flat, file, p};
    use proptest::prelude::*;

    fn set(paths: &[&str]) -> BTreeSet<RepoPath> {
        paths.iter().map(|s| p(s)).collect()
    }

    fn put(data: &str) -> FileChange {
        file(data).into()
    }

    #[test]
    fn apply_basic() {
        let base = FlatTree::from_files([(p("a"), file("1")), (p("d/x"), file("2"))]).unwrap();
        let changes = Changes::from([
            (p("a"), put("1b")),
            (p("d/x"), FileChange::Delete),
            (p("new/y"), put("3")),
        ]);
        let out = base.apply(&changes).unwrap();
        let expected =
            FlatTree::from_files([(p("a"), file("1b")), (p("new/y"), file("3"))]).unwrap();
        assert_eq!(out, expected);
        // The input is untouched.
        assert_eq!(base.len(), 2);
    }

    #[test]
    fn apply_errors() {
        let base = FlatTree::from_files([(p("a"), file("1")), (p("d/x"), file("2"))]).unwrap();
        let err = |changes: Changes| base.apply(&changes).unwrap_err();
        assert_eq!(
            err(Changes::from([(p("nope"), FileChange::Delete)])),
            ApplyError::DeleteMissing(p("nope"))
        );
        // Deleting a directory is not a thing: only files exist.
        assert_eq!(
            err(Changes::from([(p("d"), FileChange::Delete)])),
            ApplyError::DeleteMissing(p("d"))
        );
        assert_eq!(
            err(Changes::from([(p("a/b"), put("x"))])),
            ApplyError::UnderFile {
                path: p("a/b"),
                file: p("a")
            }
        );
        assert_eq!(
            err(Changes::from([(p("d"), put("x"))])),
            ApplyError::IsDirectory(p("d"))
        );
        // Two puts in one change set that conflict with each other.
        assert_eq!(
            err(Changes::from([(p("n"), put("x")), (p("n/m"), put("y"))])),
            ApplyError::UnderFile {
                path: p("n/m"),
                file: p("n")
            }
        );
    }

    #[test]
    fn apply_file_dir_swap() {
        let base = FlatTree::from_files([(p("a"), file("1")), (p("d/x"), file("2"))]).unwrap();
        let changes = Changes::from([
            (p("a"), FileChange::Delete),
            (p("a/b"), put("3")),
            (p("d/x"), FileChange::Delete),
            (p("d"), put("4")),
        ]);
        let out = base.apply(&changes).unwrap();
        assert_eq!(
            out,
            FlatTree::from_files([(p("a/b"), file("3")), (p("d"), file("4"))]).unwrap()
        );
        assert_eq!(diff(&base, &out), changes);
    }

    #[test]
    fn overlap() {
        assert!(!paths_overlap(&set(&[]), &set(&["a"])));
        assert!(paths_overlap(&set(&["a"]), &set(&["a"])));
        assert!(paths_overlap(&set(&["a"]), &set(&["a/b/c"])));
        assert!(paths_overlap(&set(&["a/b/c"]), &set(&["a"])));
        assert!(!paths_overlap(&set(&["a"]), &set(&["ab", "a.txt", "b/a"])));
        assert!(!paths_overlap(&set(&["a/b"]), &set(&["a/c", "a/b.x"])));
        assert_eq!(
            overlapping_paths(&set(&["a", "x/y", "q"]), &set(&["a/1", "a/2", "x", "z"])),
            set(&["a", "a/1", "a/2", "x", "x/y"])
        );
        let changes = Changes::from([(p("k"), FileChange::Delete)]);
        assert_eq!(changed_paths(&changes), set(&["k"]));
    }

    proptest! {
        #[test]
        fn apply_diff_round_trips(a in arb_flat(), b in arb_flat()) {
            let d = diff(&a, &b);
            prop_assert_eq!(a.apply(&d).unwrap(), b.clone());
            prop_assert!(diff(&b, &b).is_empty());
        }

        #[test]
        fn overlap_matches_brute_force(a in arb_flat(), b in arb_flat()) {
            let (a, b): (BTreeSet<_>, BTreeSet<_>) = (a.keys().cloned().collect(), b.keys().cloned().collect());
            let brute = a.iter().any(|x| b.iter().any(|y| x == y || x.is_ancestor_of(y) || y.is_ancestor_of(x)));
            prop_assert_eq!(paths_overlap(&a, &b), brute);
            prop_assert_eq!(paths_overlap(&b, &a), brute);
            prop_assert_eq!(overlapping_paths(&a, &b), overlapping_paths(&b, &a));
            prop_assert_eq!(overlapping_paths(&a, &b).is_empty(), !brute);
        }
    }
}
