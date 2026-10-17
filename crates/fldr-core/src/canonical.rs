//! Canonical JSON: the byte encoding that tree and changeset ids hash.
//!
//! This is the subset of RFC 8785 (JCS) that fldr objects need:
//! - no insignificant whitespace;
//! - object keys sorted by their UTF-16 code units (not UTF-8 bytes; the two
//!   orders differ when a key holds a character in U+E000..=U+FFFF next to a
//!   supplementary-plane character such as an emoji);
//! - integers only, within ±(2^53−1), so every value survives a round trip
//!   through an IEEE-754 double in any language; floats are rejected;
//! - strings with minimal escaping: `"` and `\`, `\b \f \n \r \t`, other
//!   control characters as lowercase `\u00xx`, everything else literal UTF-8.
//!
//! [`to_canonical`] encodes any `Serialize` value. [`from_canonical`] decodes
//! and rejects input that is not already canonical, so an object has exactly
//! one accepted encoding and therefore exactly one id.

use std::cmp::Ordering;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Number, Value};

/// Largest integer magnitude allowed in canonical JSON: 2^53 − 1.
pub const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CanonicalError {
    #[error("floating-point numbers are not allowed in canonical JSON")]
    Float,
    #[error("integer {0} is outside ±(2^53−1)")]
    IntegerOutOfRange(String),
    #[error("invalid JSON value: {0}")]
    Json(String),
    #[error("input is valid JSON but not in canonical form")]
    NotCanonical,
}

impl From<serde_json::Error> for CanonicalError {
    fn from(e: serde_json::Error) -> Self {
        CanonicalError::Json(e.to_string())
    }
}

/// Encodes `value` as canonical JSON bytes.
pub fn to_canonical<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, CanonicalError> {
    let value = serde_json::to_value(value)?;
    let mut out = Vec::new();
    write_value(&value, &mut out)?;
    Ok(out)
}

/// Decodes canonical JSON bytes, rejecting anything that does not re-encode
/// to exactly the same bytes (extra whitespace, unsorted keys, unknown
/// fields, non-minimal escapes, floats, …).
pub fn from_canonical<T: Serialize + DeserializeOwned>(bytes: &[u8]) -> Result<T, CanonicalError> {
    let value: T = serde_json::from_slice(bytes)?;
    if to_canonical(&value)? != bytes {
        return Err(CanonicalError::NotCanonical);
    }
    Ok(value)
}

/// True if `bytes` is the canonical encoding of some `T`.
pub fn is_canonical<T: Serialize + DeserializeOwned>(bytes: &[u8]) -> bool {
    from_canonical::<T>(bytes).is_ok()
}

/// Orders strings by UTF-16 code units, as RFC 8785 §3.2.3 requires.
pub fn cmp_utf16(a: &str, b: &str) -> Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

fn write_value(value: &Value, out: &mut Vec<u8>) -> Result<(), CanonicalError> {
    match value {
        Value::Null => out.extend_from_slice(b"null"),
        Value::Bool(true) => out.extend_from_slice(b"true"),
        Value::Bool(false) => out.extend_from_slice(b"false"),
        Value::Number(n) => write_number(n, out)?,
        Value::String(s) => write_string(s, out),
        Value::Array(items) => {
            out.push(b'[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write_value(item, out)?;
            }
            out.push(b']');
        }
        Value::Object(map) => {
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_by(|a, b| cmp_utf16(a.0, b.0));
            out.push(b'{');
            for (i, (k, v)) in entries.into_iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write_string(k, out);
                out.push(b':');
                write_value(v, out)?;
            }
            out.push(b'}');
        }
    }
    Ok(())
}

fn write_number(n: &Number, out: &mut Vec<u8>) -> Result<(), CanonicalError> {
    if let Some(u) = n.as_u64() {
        if u > MAX_SAFE_INTEGER {
            return Err(CanonicalError::IntegerOutOfRange(u.to_string()));
        }
        out.extend_from_slice(u.to_string().as_bytes());
    } else if let Some(i) = n.as_i64() {
        if i.unsigned_abs() > MAX_SAFE_INTEGER {
            return Err(CanonicalError::IntegerOutOfRange(i.to_string()));
        }
        out.extend_from_slice(i.to_string().as_bytes());
    } else {
        return Err(CanonicalError::Float);
    }
    Ok(())
}

fn write_string(s: &str, out: &mut Vec<u8>) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.push(b'"');
    for c in s.chars() {
        match c {
            '"' => out.extend_from_slice(b"\\\""),
            '\\' => out.extend_from_slice(b"\\\\"),
            '\u{8}' => out.extend_from_slice(b"\\b"),
            '\u{c}' => out.extend_from_slice(b"\\f"),
            '\n' => out.extend_from_slice(b"\\n"),
            '\r' => out.extend_from_slice(b"\\r"),
            '\t' => out.extend_from_slice(b"\\t"),
            c if (c as u32) < 0x20 => {
                let b = c as u8;
                out.extend_from_slice(b"\\u00");
                out.push(HEX[(b >> 4) as usize]);
                out.push(HEX[(b & 0xf) as usize]);
            }
            c => {
                let mut buf = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
        }
    }
    out.push(b'"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use serde_json::json;
    use std::collections::BTreeMap;

    fn enc(v: &Value) -> String {
        String::from_utf8(to_canonical(v).unwrap()).unwrap()
    }

    #[test]
    fn no_whitespace_and_sorted_keys() {
        let v = json!({ "b": [1, 2, {"z": null, "a": true}], "a": "x", "c": -5 });
        assert_eq!(enc(&v), r#"{"a":"x","b":[1,2,{"a":true,"z":null}],"c":-5}"#);
    }

    #[test]
    fn keys_sort_by_utf16_not_bytes() {
        // U+FF61 is one UTF-16 unit 0xFF61; U+1F600 is the surrogate pair
        // 0xD83D 0xDE00. In UTF-16 the emoji sorts first; in UTF-8 bytes
        // (EF BD A1 vs F0 9F 98 80) it sorts last.
        assert!("\u{ff61}".as_bytes() < "😀".as_bytes());
        assert_eq!(cmp_utf16("\u{ff61}", "😀"), Ordering::Greater);
        let v = json!({ "\u{ff61}": 1, "😀": 2, "a": 3 });
        assert_eq!(enc(&v), "{\"a\":3,\"😀\":2,\"\u{ff61}\":1}");
    }

    #[test]
    fn rfc8785_key_sorting_example() {
        // RFC 8785 §3.2.3 example keys.
        let v = json!({
            "\u{20ac}": "Euro Sign",
            "\r": "Carriage Return",
            "\u{fb33}": "Hebrew Letter Dalet With Dagesh",
            "1": "One",
            "\u{1f600}": "Emoji: Grinning Face",
            "\u{80}": "Control",
            "\u{f6}": "Latin Small Letter O With Diaeresis"
        });
        let expected = concat!(
            "{\"\\r\":\"Carriage Return\",",
            "\"1\":\"One\",",
            "\"\u{80}\":\"Control\",",
            "\"\u{f6}\":\"Latin Small Letter O With Diaeresis\",",
            "\"\u{20ac}\":\"Euro Sign\",",
            "\"\u{1f600}\":\"Emoji: Grinning Face\",",
            "\"\u{fb33}\":\"Hebrew Letter Dalet With Dagesh\"}",
        );
        assert_eq!(enc(&v), expected);
    }

    #[test]
    fn string_escaping_is_minimal() {
        let v = json!("a\"b\\c\u{8}\u{c}\n\r\t\u{1}\u{1f}/\u{7f}é😀");
        assert_eq!(
            enc(&v),
            "\"a\\\"b\\\\c\\b\\f\\n\\r\\t\\u0001\\u001f/\u{7f}é😀\""
        );
    }

    #[test]
    fn integers_in_range_only() {
        let max = MAX_SAFE_INTEGER as i64;
        assert_eq!(enc(&json!(max)), "9007199254740991");
        assert_eq!(enc(&json!(-max)), "-9007199254740991");
        assert_eq!(enc(&json!(0)), "0");
        assert!(matches!(
            to_canonical(&json!(max + 1)),
            Err(CanonicalError::IntegerOutOfRange(_))
        ));
        assert!(matches!(
            to_canonical(&json!(-max - 1)),
            Err(CanonicalError::IntegerOutOfRange(_))
        ));
        assert!(matches!(
            to_canonical(&u64::MAX),
            Err(CanonicalError::IntegerOutOfRange(_))
        ));
        assert!(matches!(
            to_canonical(&i64::MIN),
            Err(CanonicalError::IntegerOutOfRange(_))
        ));
    }

    #[test]
    fn floats_rejected() {
        assert_eq!(to_canonical(&1.5f64), Err(CanonicalError::Float));
        assert_eq!(to_canonical(&1.0f64), Err(CanonicalError::Float));
        assert_eq!(
            to_canonical(&json!({"a": [0.1]})),
            Err(CanonicalError::Float)
        );
    }

    #[test]
    fn from_canonical_rejects_non_canonical() {
        type M = BTreeMap<String, i64>;
        assert_eq!(from_canonical::<M>(br#"{"a":1,"b":2}"#).unwrap().len(), 2);
        for bad in [
            &br#"{"b":2,"a":1}"#[..],
            br#"{ "a":1}"#,
            br#"{"a":1,"b":2} "#,
            br#"{"\u0061":1}"#,
            br#"{"a":1.0}"#,
            br#"{"a":01}"#,
        ] {
            assert!(
                from_canonical::<M>(bad).is_err(),
                "{}",
                String::from_utf8_lossy(bad)
            );
        }
        assert!(is_canonical::<Value>(br#"[1,"x",null]"#));
        assert!(!is_canonical::<Value>(br#"[1, "x"]"#));
    }

    proptest! {
        #[test]
        fn string_round_trips(s in any::<String>()) {
            let bytes = to_canonical(&s).unwrap();
            // Matches serde_json's escaping exactly.
            prop_assert_eq!(&bytes, &serde_json::to_vec(&s).unwrap());
            prop_assert_eq!(from_canonical::<String>(&bytes).unwrap(), s);
        }

        #[test]
        fn maps_are_stable(m in prop::collection::btree_map(any::<String>(), -1000i64..1000, 0..8)) {
            let bytes = to_canonical(&m).unwrap();
            prop_assert_eq!(&to_canonical(&m).unwrap(), &bytes);
            let back: Value = serde_json::from_slice(&bytes).unwrap();
            prop_assert_eq!(&to_canonical(&back).unwrap(), &bytes);
            prop_assert!(is_canonical::<Value>(&bytes));
        }
    }
}
