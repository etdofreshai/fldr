//! Content hashes and typed object ids.
//!
//! A [`BlobId`] is the plain SHA-256 of a file's bytes, so `sha256sum` on a
//! local file yields its fldr blob id. Trees and changesets are hashed over
//! their canonical encoding with a type prefix (`"tree\0"`, `"changeset\0"`),
//! see [`Hash::of_typed`]. Ids are typed so a tree id can never be looked up
//! as a blob id by accident.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};

/// A SHA-256 digest. Serializes as 64 lowercase hex characters.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Hash([u8; 32]);

impl Hash {
    pub const LEN: usize = 32;
    pub const HEX_LEN: usize = 64;

    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Plain SHA-256 of `data`.
    pub fn of(data: &[u8]) -> Self {
        Self(Sha256::digest(data).into())
    }

    /// SHA-256 of `kind.prefix() || data`. Used for structured objects.
    pub fn of_typed(kind: ObjectKind, data: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(kind.prefix());
        hasher.update(data);
        Self(hasher.finalize().into())
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    /// The first `n` hex characters, for display (`n` is capped at 64).
    pub fn short(&self, n: usize) -> String {
        let mut s = self.to_hex();
        s.truncate(n.min(Self::HEX_LEN));
        s
    }
}

impl fmt::Display for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl fmt::Debug for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Hash({})", self.short(12))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HashParseError {
    #[error("hash must be {expected} hex characters, got {actual}")]
    Length { expected: usize, actual: usize },
    #[error("hash must be lowercase hex")]
    NotLowercaseHex,
}

impl FromStr for Hash {
    type Err = HashParseError;

    /// Accepts exactly 64 lowercase hex characters, so every hash has one
    /// spelling and string comparisons agree with byte comparisons.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len() != Self::HEX_LEN {
            return Err(HashParseError::Length {
                expected: Self::HEX_LEN,
                actual: s.len(),
            });
        }
        if !s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return Err(HashParseError::NotLowercaseHex);
        }
        let mut out = [0u8; 32];
        hex::decode_to_slice(s, &mut out).map_err(|_| HashParseError::NotLowercaseHex)?;
        Ok(Self(out))
    }
}

impl Serialize for Hash {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Hash {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// The kind of a structured, content-addressed object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ObjectKind {
    Tree,
    Changeset,
}

impl ObjectKind {
    /// Domain-separation prefix hashed before the object's canonical bytes.
    pub const fn prefix(self) -> &'static [u8] {
        match self {
            ObjectKind::Tree => b"tree\0",
            ObjectKind::Changeset => b"changeset\0",
        }
    }
}

macro_rules! typed_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub Hash);

        impl $name {
            pub const fn hash(&self) -> &Hash {
                &self.0
            }

            pub fn to_hex(&self) -> String {
                self.0.to_hex()
            }

            pub fn short(&self, n: usize) -> String {
                self.0.short(n)
            }
        }

        impl From<Hash> for $name {
            fn from(hash: Hash) -> Self {
                Self(hash)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, f)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self.0.short(12))
            }
        }

        impl FromStr for $name {
            type Err = HashParseError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                s.parse().map(Self)
            }
        }
    };
}

typed_id!(
    /// Id of a file's contents: the plain SHA-256 of its bytes.
    BlobId
);
typed_id!(
    /// Id of a directory listing: `Hash::of_typed(ObjectKind::Tree, canonical)`.
    TreeId
);
typed_id!(
    /// Id of a changeset: `Hash::of_typed(ObjectKind::Changeset, canonical)`.
    ChangesetId
);

impl BlobId {
    /// The blob id of `data`.
    pub fn of(data: &[u8]) -> Self {
        Self(Hash::of(data))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blob_id_matches_sha256sum() {
        // printf 'hello\n' | sha256sum
        assert_eq!(
            BlobId::of(b"hello\n").to_hex(),
            "5891b5b522d5df086d0ff0b110fbd9d21bb4fc7163af34d08286a2e846f6be03"
        );
    }

    #[test]
    fn typed_hash_is_domain_separated() {
        let data = b"{}";
        assert_ne!(Hash::of(data), Hash::of_typed(ObjectKind::Tree, data));
        assert_ne!(
            Hash::of_typed(ObjectKind::Tree, data),
            Hash::of_typed(ObjectKind::Changeset, data)
        );
    }

    #[test]
    fn hex_round_trip_and_strict_parsing() {
        let h = Hash::of(b"x");
        assert_eq!(h.to_hex().parse::<Hash>().unwrap(), h);
        assert!(h.to_hex().to_uppercase().parse::<Hash>().is_err());
        assert!("abc".parse::<Hash>().is_err());
    }

    #[test]
    fn serde_uses_hex_strings() {
        let id = TreeId(Hash::of(b"t"));
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, format!("\"{}\"", id.to_hex()));
        assert_eq!(serde_json::from_str::<TreeId>(&json).unwrap(), id);
    }
}
