//! Repository-relative paths.
//!
//! A [`RepoPath`] is a relative, `/`-separated UTF-8 path such as
//! `src/main.rs`. It is validated on construction and on deserialization, so
//! every `RepoPath` in memory is well formed: no empty, `.` or `..`
//! components, no leading or trailing `/`, no NUL or backslash, components of
//! at most [`MAX_COMPONENT_LEN`] bytes and at most [`MAX_PATH_LEN`] bytes in
//! total. The root directory itself is not a `RepoPath`.

use std::borrow::Borrow;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Maximum length of one path component, in bytes.
pub const MAX_COMPONENT_LEN: usize = 255;
/// Maximum length of a whole path, in bytes.
pub const MAX_PATH_LEN: usize = 4096;

/// Why a string is not a valid [`RepoPath`] or path component.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PathError {
    #[error("path is empty")]
    Empty,
    #[error("path must be relative (no leading '/')")]
    LeadingSlash,
    #[error("path must not end with '/'")]
    TrailingSlash,
    #[error("path has an empty component ('//')")]
    EmptyComponent,
    #[error("path has a '.' or '..' component")]
    DotComponent,
    #[error("path contains a NUL byte")]
    Nul,
    #[error("path contains a backslash")]
    Backslash,
    #[error("path component is {len} bytes, max {MAX_COMPONENT_LEN}")]
    ComponentTooLong { len: usize },
    #[error("path is {len} bytes, max {MAX_PATH_LEN}")]
    TooLong { len: usize },
    #[error("path component must not contain '/'")]
    SlashInComponent,
}

/// Checks that `name` is a valid single path component (a tree entry name).
pub fn validate_component(name: &str) -> Result<(), PathError> {
    if name.is_empty() {
        return Err(PathError::EmptyComponent);
    }
    if name == "." || name == ".." {
        return Err(PathError::DotComponent);
    }
    if name.len() > MAX_COMPONENT_LEN {
        return Err(PathError::ComponentTooLong { len: name.len() });
    }
    for b in name.bytes() {
        match b {
            0 => return Err(PathError::Nul),
            b'\\' => return Err(PathError::Backslash),
            b'/' => return Err(PathError::SlashInComponent),
            _ => {}
        }
    }
    Ok(())
}

fn validate(s: &str) -> Result<(), PathError> {
    if s.is_empty() {
        return Err(PathError::Empty);
    }
    if s.len() > MAX_PATH_LEN {
        return Err(PathError::TooLong { len: s.len() });
    }
    if s.starts_with('/') {
        return Err(PathError::LeadingSlash);
    }
    if s.ends_with('/') {
        return Err(PathError::TrailingSlash);
    }
    s.split('/').try_for_each(validate_component)
}

/// A validated, repository-relative path. Serializes as a plain string.
///
/// Ordering is by UTF-8 bytes (the same as `str` ordering), so a
/// `BTreeMap<RepoPath, _>` iterates in byte order. Note that this is *not*
/// the key order of canonical JSON, which sorts by UTF-16 code units; the
/// canonical encoder re-sorts keys itself.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RepoPath(String);

impl RepoPath {
    /// Validates `s` and wraps it.
    pub fn new(s: impl Into<String>) -> Result<Self, PathError> {
        let s = s.into();
        validate(&s)?;
        Ok(Self(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }

    /// The containing directory, or `None` for a top-level path.
    pub fn parent(&self) -> Option<RepoPath> {
        self.0.rfind('/').map(|i| RepoPath(self.0[..i].to_owned()))
    }

    /// The path's components, outermost first.
    pub fn components(&self) -> impl DoubleEndedIterator<Item = &str> + '_ {
        self.0.split('/')
    }

    /// The last component.
    pub fn file_name(&self) -> &str {
        match self.0.rfind('/') {
            Some(i) => &self.0[i + 1..],
            None => &self.0,
        }
    }

    /// Number of components (1 for a top-level path).
    pub fn depth(&self) -> usize {
        self.0.bytes().filter(|&b| b == b'/').count() + 1
    }

    /// Appends a relative path (one or more components), validating the
    /// result.
    pub fn join(&self, rel: &str) -> Result<RepoPath, PathError> {
        validate(rel)?;
        RepoPath::new(format!("{}/{}", self.0, rel))
    }

    /// Strict ancestors, innermost first: `a/b/c` yields `a/b`, then `a`.
    pub fn ancestors(&self) -> impl Iterator<Item = &str> + '_ {
        self.0
            .char_indices()
            .rev()
            .filter(|&(_, c)| c == '/')
            .map(|(i, _)| &self.0[..i])
    }

    /// True if `self` is a strict ancestor directory of `other`: `a` is an
    /// ancestor of `a/b` and `a/b/c`, but not of `a` itself or of `ab`.
    pub fn is_ancestor_of(&self, other: &RepoPath) -> bool {
        other.0.len() > self.0.len()
            && other.0.starts_with(&self.0)
            && other.0.as_bytes()[self.0.len()] == b'/'
    }
}

impl fmt::Display for RepoPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for RepoPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RepoPath({:?})", self.0)
    }
}

impl FromStr for RepoPath {
    type Err = PathError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        RepoPath::new(s)
    }
}

impl TryFrom<String> for RepoPath {
    type Error = PathError;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        RepoPath::new(s)
    }
}

impl TryFrom<&str> for RepoPath {
    type Error = PathError;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        RepoPath::new(s)
    }
}

impl AsRef<str> for RepoPath {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// Lets `BTreeMap<RepoPath, _>` be queried with a `&str`. Sound because
/// `RepoPath` orders exactly like its string.
impl Borrow<str> for RepoPath {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl Serialize for RepoPath {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for RepoPath {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        RepoPath::new(s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn p(s: &str) -> RepoPath {
        RepoPath::new(s).unwrap()
    }

    #[test]
    fn accepts_valid_paths() {
        for s in [
            "a",
            "a/b",
            "src/main.rs",
            ".hidden",
            "...",
            "a b/c d",
            "é/😀",
        ] {
            assert_eq!(p(s).as_str(), s);
        }
    }

    #[test]
    fn rejects_invalid_paths() {
        let cases: &[(&str, PathError)] = &[
            ("", PathError::Empty),
            ("/a", PathError::LeadingSlash),
            ("a/", PathError::TrailingSlash),
            ("/", PathError::LeadingSlash),
            ("a//b", PathError::EmptyComponent),
            (".", PathError::DotComponent),
            ("a/../b", PathError::DotComponent),
            ("a/./b", PathError::DotComponent),
            ("a\0b", PathError::Nul),
            ("a\\b", PathError::Backslash),
        ];
        for (s, err) in cases {
            assert_eq!(RepoPath::new(*s).unwrap_err(), *err, "{s:?}");
        }
        let long = "x".repeat(256);
        assert_eq!(
            RepoPath::new(long).unwrap_err(),
            PathError::ComponentTooLong { len: 256 }
        );
        assert!(RepoPath::new("x".repeat(255)).is_ok());
        let deep = vec!["abcdefg"; 513].join("/"); // 513*8-1 = 4103 bytes
        assert_eq!(
            RepoPath::new(deep).unwrap_err(),
            PathError::TooLong { len: 4103 }
        );
        let max = vec!["abcdefg"; 512].join("/") + "x"; // 4096 bytes
        assert!(RepoPath::new(max).is_ok());
    }

    #[test]
    fn accessors() {
        let path = p("a/b/c.txt");
        assert_eq!(path.parent(), Some(p("a/b")));
        assert_eq!(p("a").parent(), None);
        assert_eq!(path.file_name(), "c.txt");
        assert_eq!(p("top").file_name(), "top");
        assert_eq!(path.components().collect::<Vec<_>>(), ["a", "b", "c.txt"]);
        assert_eq!(path.depth(), 3);
        assert_eq!(path.ancestors().collect::<Vec<_>>(), ["a/b", "a"]);
        assert_eq!(p("a").join("b/c").unwrap(), p("a/b/c"));
        assert!(p("a").join("../b").is_err());
        assert!(p("a").join("").is_err());
    }

    #[test]
    fn ancestry() {
        assert!(p("a").is_ancestor_of(&p("a/b")));
        assert!(p("a").is_ancestor_of(&p("a/b/c")));
        assert!(!p("a").is_ancestor_of(&p("a")));
        assert!(!p("a").is_ancestor_of(&p("ab")));
        assert!(!p("a").is_ancestor_of(&p("ab/c")));
        assert!(!p("a/b").is_ancestor_of(&p("a")));
    }

    #[test]
    fn orders_by_bytes() {
        // '.' (0x2e) < '/' (0x2f) < '0' (0x30)
        let mut v = [p("a0"), p("a/b"), p("a.txt"), p("\u{ff61}"), p("😀")];
        v.sort();
        let s: Vec<_> = v.iter().map(RepoPath::as_str).collect();
        assert_eq!(s, ["a.txt", "a/b", "a0", "\u{ff61}", "😀"]);
    }

    #[test]
    fn serde_validates() {
        assert_eq!(serde_json::to_string(&p("a/b")).unwrap(), "\"a/b\"");
        assert_eq!(
            serde_json::from_str::<RepoPath>("\"a/b\"").unwrap(),
            p("a/b")
        );
        assert!(serde_json::from_str::<RepoPath>("\"a/../b\"").is_err());
        assert!(serde_json::from_str::<RepoPath>("\"/a\"").is_err());
    }

    fn component() -> impl Strategy<Value = String> {
        "[^/\\\\\0]{1,20}".prop_filter("dot components", |s| s != "." && s != "..")
    }

    proptest! {
        #[test]
        fn valid_paths_round_trip(parts in prop::collection::vec(component(), 1..6)) {
            let s = parts.join("/");
            let path = RepoPath::new(s.clone()).unwrap();
            prop_assert_eq!(path.as_str(), s.as_str());
            prop_assert_eq!(path.components().collect::<Vec<_>>(), parts.iter().map(String::as_str).collect::<Vec<_>>());
            let json = serde_json::to_string(&path).unwrap();
            prop_assert_eq!(serde_json::from_str::<RepoPath>(&json).unwrap(), path.clone());
            if let Some(parent) = path.parent() {
                prop_assert!(parent.is_ancestor_of(&path));
                prop_assert_eq!(parent.join(path.file_name()).unwrap(), path);
            }
        }

        #[test]
        fn validation_matches_spec(s in "[a./\\\\\0]{0,12}") {
            let expected_ok = !s.is_empty()
                && s.split('/').all(|c| !c.is_empty() && c != "." && c != ".." && !c.contains(['\0', '\\']));
            prop_assert_eq!(RepoPath::new(s.clone()).is_ok(), expected_ok, "{:?}", s);
            prop_assert_eq!(serde_json::from_value::<RepoPath>(serde_json::Value::String(s)).is_ok(), expected_ok);
        }
    }
}
