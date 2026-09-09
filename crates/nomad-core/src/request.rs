//! Encode and decode NomadNet link request payloads.
//!
//! Covers form `field_*` / `var_*` MessagePack maps and the `/media` request
//! body (`path` + `key`) used by NomadNet 1.4.1 in-page WebP fetches.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::NomadError;

/// Max MessagePack body accepted by [`decode_request_fields`].
pub const MAX_REQUEST_BODY_BYTES: usize = 64 * 1024;
/// Max map entries retained after decode.
pub const MAX_REQUEST_FIELDS: usize = 64;
/// Max UTF-8 bytes for a single map key.
pub const MAX_REQUEST_FIELD_KEY_BYTES: usize = 256;
/// Max UTF-8 bytes for a single map value.
pub const MAX_REQUEST_FIELD_VALUE_BYTES: usize = 4 * 1024;
/// Max MessagePack nesting depth while decoding.
pub const MAX_REQUEST_MSGPACK_DEPTH: usize = 8;

/// Parsed request fields from a Nomad page form submission.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct NomadRequestFields {
    /// Original MessagePack (or empty) bytes.
    #[serde(skip)]
    pub raw: Vec<u8>,
    /// String keys (e.g. `field_*`, `var_*`) mapped to string values.
    ///
    /// Decoding accepts any string→string map entry; prefix filtering is left
    /// to the caller.
    pub fields: BTreeMap<String, String>,
}

/// Encode request fields into MessagePack map bytes for a Link REQUEST body.
///
/// Empty input yields an empty `Vec` (no MessagePack envelope). Entries that
/// exceed [`MAX_REQUEST_FIELD_KEY_BYTES`] / [`MAX_REQUEST_FIELD_VALUE_BYTES`]
/// are skipped; at most [`MAX_REQUEST_FIELDS`] entries are retained (same soft
/// caps as [`decode_request_fields`]). Encode failures yield an empty `Vec`.
pub fn encode_request_fields(fields: &BTreeMap<String, String>) -> Vec<u8> {
    if fields.is_empty() {
        return Vec::new();
    }
    let mut map = Vec::new();
    for (key, value) in fields.iter().take(MAX_REQUEST_FIELDS) {
        if key.len() > MAX_REQUEST_FIELD_KEY_BYTES || value.len() > MAX_REQUEST_FIELD_VALUE_BYTES {
            continue;
        }
        map.push((
            rmpv::Value::String(key.as_str().into()),
            rmpv::Value::String(value.as_str().into()),
        ));
    }
    if map.is_empty() {
        return Vec::new();
    }
    let mut buf = Vec::new();
    if rmpv::encode::write_value(&mut buf, &rmpv::Value::Map(map)).is_err() {
        return Vec::new();
    }
    buf
}

/// Decode request body bytes into typed fields.
///
/// Accepts a MessagePack map of string→string (canonical NomadNet form data).
/// Non-map or empty input yields an empty field map with `raw` preserved when
/// the body is within [`MAX_REQUEST_BODY_BYTES`]. Oversized bodies return
/// [`NomadError::TooLarge`].
pub fn decode_request_fields(data: &[u8]) -> Result<NomadRequestFields, NomadError> {
    if data.len() > MAX_REQUEST_BODY_BYTES {
        return Err(NomadError::TooLarge {
            size: data.len(),
            max: MAX_REQUEST_BODY_BYTES,
        });
    }
    let mut out = NomadRequestFields {
        raw: data.to_vec(),
        fields: BTreeMap::new(),
    };
    if data.is_empty() {
        return Ok(out);
    }
    let Ok(value) = rmpv::decode::read_value_with_max_depth(&mut &*data, MAX_REQUEST_MSGPACK_DEPTH)
    else {
        return Ok(out);
    };
    let rmpv::Value::Map(map) = value else {
        return Ok(out);
    };
    for (key, val) in map.into_iter().take(MAX_REQUEST_FIELDS) {
        let Some(k) = value_as_string(&key) else {
            continue;
        };
        let Some(v) = value_as_string(&val) else {
            continue;
        };
        if k.len() > MAX_REQUEST_FIELD_KEY_BYTES || v.len() > MAX_REQUEST_FIELD_VALUE_BYTES {
            continue;
        }
        out.fields.insert(k, v);
    }
    Ok(out)
}

fn value_as_string(value: &rmpv::Value) -> Option<String> {
    match value {
        rmpv::Value::String(s) => s.as_str().map(str::to_owned),
        rmpv::Value::Binary(b) => String::from_utf8(b.clone()).ok(),
        rmpv::Value::Boolean(b) => Some(b.to_string()),
        rmpv::Value::Integer(i) => i.as_i64().map(|n| n.to_string()),
        rmpv::Value::F64(f) => Some(f.to_string()),
        _ => None,
    }
}

/// Decoded `/media` request body (`path` required string; `key` present, often Nil).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaRequest {
    /// Relative media path under `pages/` (may include a `/media/` prefix).
    pub path: String,
}

/// Encode a NomadNet `/media` request body: msgpack map `{path, key: nil}`.
pub fn encode_media_request(path: &str) -> Vec<u8> {
    let map = vec![
        (
            rmpv::Value::String("path".into()),
            rmpv::Value::String(path.into()),
        ),
        (rmpv::Value::String("key".into()), rmpv::Value::Nil),
    ];
    let mut buf = Vec::new();
    if rmpv::encode::write_value(&mut buf, &rmpv::Value::Map(map)).is_err() {
        return Vec::new();
    }
    buf
}

/// Decode a `/media` request body.
///
/// Requires a MessagePack map containing **both** `"path"` (string) and `"key"`
/// (any value, including Nil). Missing either key, non-map input, or a non-string
/// `path` yields [`NomadError::InvalidPath`] (caller should Drop).
pub fn decode_media_request(data: &[u8]) -> Result<MediaRequest, NomadError> {
    if data.len() > MAX_REQUEST_BODY_BYTES {
        return Err(NomadError::TooLarge {
            size: data.len(),
            max: MAX_REQUEST_BODY_BYTES,
        });
    }
    let value = rmpv::decode::read_value_with_max_depth(&mut &*data, MAX_REQUEST_MSGPACK_DEPTH)
        .map_err(|_| NomadError::InvalidPath("media request is not valid msgpack".into()))?;
    let rmpv::Value::Map(map) = value else {
        return Err(NomadError::InvalidPath(
            "media request must be a msgpack map".into(),
        ));
    };

    let mut path: Option<String> = None;
    let mut has_key = false;
    for (k, v) in map {
        let Some(name) = value_as_string(&k) else {
            continue;
        };
        match name.as_str() {
            "path" => {
                let Some(p) = value_as_string(&v) else {
                    return Err(NomadError::InvalidPath(
                        "media request path must be a string".into(),
                    ));
                };
                path = Some(p);
            }
            "key" => has_key = true,
            _ => {}
        }
    }
    if !has_key {
        return Err(NomadError::InvalidPath("media request missing key".into()));
    }
    let Some(path) = path else {
        return Err(NomadError::InvalidPath("media request missing path".into()));
    };
    Ok(MediaRequest { path })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_empty_map_yields_empty_bytes() {
        assert!(encode_request_fields(&BTreeMap::new()).is_empty());
    }

    #[test]
    fn encode_round_trips_through_decode() {
        let mut fields = BTreeMap::new();
        fields.insert("field_q".to_string(), "hello".to_string());
        fields.insert("var_mode".to_string(), "search".to_string());
        let encoded = encode_request_fields(&fields);
        assert!(!encoded.is_empty());
        let parsed = decode_request_fields(&encoded).unwrap();
        assert_eq!(parsed.fields, fields);
    }

    #[test]
    fn encode_skips_oversized_key_and_value() {
        let mut fields = BTreeMap::new();
        fields.insert("ok".to_string(), "v".to_string());
        fields.insert("k".repeat(MAX_REQUEST_FIELD_KEY_BYTES + 1), "v".to_string());
        fields.insert(
            "field_big".to_string(),
            "v".repeat(MAX_REQUEST_FIELD_VALUE_BYTES + 1),
        );
        let encoded = encode_request_fields(&fields);
        let parsed = decode_request_fields(&encoded).unwrap();
        assert_eq!(parsed.fields.len(), 1);
        assert_eq!(parsed.fields.get("ok").map(String::as_str), Some("v"));
    }

    #[test]
    fn encode_caps_field_count() {
        let mut fields = BTreeMap::new();
        for i in 0..MAX_REQUEST_FIELDS + 10 {
            fields.insert(format!("field_{i:03}"), "v".to_string());
        }
        let encoded = encode_request_fields(&fields);
        let parsed = decode_request_fields(&encoded).unwrap();
        assert_eq!(parsed.fields.len(), MAX_REQUEST_FIELDS);
    }

    #[test]
    fn decodes_msgpack_string_map() {
        let map = vec![
            (
                rmpv::Value::String("field_q".into()),
                rmpv::Value::String("hello".into()),
            ),
            (
                rmpv::Value::String("var_mode".into()),
                rmpv::Value::String("search".into()),
            ),
        ];
        let mut buf = Vec::new();
        rmpv::encode::write_value(&mut buf, &rmpv::Value::Map(map)).unwrap();
        let parsed = decode_request_fields(&buf).unwrap();
        assert_eq!(
            parsed.fields.get("field_q").map(String::as_str),
            Some("hello")
        );
        assert_eq!(
            parsed.fields.get("var_mode").map(String::as_str),
            Some("search")
        );
    }

    #[test]
    fn empty_on_invalid() {
        let parsed = decode_request_fields(b"not-msgpack").unwrap();
        assert!(parsed.fields.is_empty());
        assert_eq!(parsed.raw, b"not-msgpack");
    }

    #[test]
    fn empty_body_yields_empty_fields() {
        let parsed = decode_request_fields(b"").unwrap();
        assert!(parsed.fields.is_empty());
        assert!(parsed.raw.is_empty());
    }

    #[test]
    fn non_map_yields_empty_fields() {
        let mut buf = Vec::new();
        rmpv::encode::write_value(&mut buf, &rmpv::Value::Array(vec![])).unwrap();
        let parsed = decode_request_fields(&buf).unwrap();
        assert!(parsed.fields.is_empty());
    }

    #[test]
    fn coerces_integer_and_bool_values() {
        let map = vec![
            (
                rmpv::Value::String("field_n".into()),
                rmpv::Value::Integer(42.into()),
            ),
            (
                rmpv::Value::String("field_b".into()),
                rmpv::Value::Boolean(true),
            ),
        ];
        let mut buf = Vec::new();
        rmpv::encode::write_value(&mut buf, &rmpv::Value::Map(map)).unwrap();
        let parsed = decode_request_fields(&buf).unwrap();
        assert_eq!(parsed.fields.get("field_n").map(String::as_str), Some("42"));
        assert_eq!(
            parsed.fields.get("field_b").map(String::as_str),
            Some("true")
        );
    }

    #[test]
    fn rejects_oversized_body() {
        let big = vec![0u8; MAX_REQUEST_BODY_BYTES + 1];
        let err = decode_request_fields(&big).unwrap_err();
        assert!(matches!(err, NomadError::TooLarge { .. }));
    }

    #[test]
    fn caps_field_count() {
        let map: Vec<_> = (0..MAX_REQUEST_FIELDS + 10)
            .map(|i| {
                (
                    rmpv::Value::String(format!("field_{i}").into()),
                    rmpv::Value::String("v".into()),
                )
            })
            .collect();
        let mut buf = Vec::new();
        rmpv::encode::write_value(&mut buf, &rmpv::Value::Map(map)).unwrap();
        let parsed = decode_request_fields(&buf).unwrap();
        assert_eq!(parsed.fields.len(), MAX_REQUEST_FIELDS);
    }

    #[test]
    fn coerces_binary_and_f64_values() {
        let map = vec![
            (
                rmpv::Value::String("field_bin".into()),
                rmpv::Value::Binary(b"utf8-ok".to_vec()),
            ),
            (rmpv::Value::String("field_f".into()), rmpv::Value::F64(1.5)),
            (
                rmpv::Value::String("field_bad_bin".into()),
                rmpv::Value::Binary(vec![0xff, 0xfe]),
            ),
        ];
        let mut buf = Vec::new();
        rmpv::encode::write_value(&mut buf, &rmpv::Value::Map(map)).unwrap();
        let parsed = decode_request_fields(&buf).unwrap();
        assert_eq!(
            parsed.fields.get("field_bin").map(String::as_str),
            Some("utf8-ok")
        );
        assert_eq!(
            parsed.fields.get("field_f").map(String::as_str),
            Some("1.5")
        );
        assert!(!parsed.fields.contains_key("field_bad_bin"));
    }

    #[test]
    fn skips_oversized_key_and_value() {
        let map = vec![
            (
                rmpv::Value::String("ok".into()),
                rmpv::Value::String("v".into()),
            ),
            (
                rmpv::Value::String("k".repeat(MAX_REQUEST_FIELD_KEY_BYTES + 1).into()),
                rmpv::Value::String("v".into()),
            ),
            (
                rmpv::Value::String("field_big".into()),
                rmpv::Value::String("v".repeat(MAX_REQUEST_FIELD_VALUE_BYTES + 1).into()),
            ),
        ];
        let mut buf = Vec::new();
        rmpv::encode::write_value(&mut buf, &rmpv::Value::Map(map)).unwrap();
        let parsed = decode_request_fields(&buf).unwrap();
        assert_eq!(parsed.fields.len(), 1);
        assert_eq!(parsed.fields.get("ok").map(String::as_str), Some("v"));
    }

    #[test]
    fn deeply_nested_msgpack_yields_empty_fields() {
        // Nest maps deeper than MAX_REQUEST_MSGPACK_DEPTH so decode fails soft.
        let mut inner = rmpv::Value::String("leaf".into());
        for _ in 0..=MAX_REQUEST_MSGPACK_DEPTH {
            inner = rmpv::Value::Map(vec![(rmpv::Value::String("k".into()), inner)]);
        }
        let mut buf = Vec::new();
        rmpv::encode::write_value(&mut buf, &inner).unwrap();
        let parsed = decode_request_fields(&buf).unwrap();
        assert!(parsed.fields.is_empty());
        assert_eq!(parsed.raw, buf);
    }

    #[test]
    fn media_request_round_trips_with_nil_key() {
        let encoded = encode_media_request("header.webp");
        let parsed = decode_media_request(&encoded).unwrap();
        assert_eq!(parsed.path, "header.webp");
    }

    #[test]
    fn media_request_requires_key_field() {
        let map = vec![(
            rmpv::Value::String("path".into()),
            rmpv::Value::String("a.webp".into()),
        )];
        let mut buf = Vec::new();
        rmpv::encode::write_value(&mut buf, &rmpv::Value::Map(map)).unwrap();
        let err = decode_media_request(&buf).unwrap_err();
        assert!(matches!(err, NomadError::InvalidPath(_)));
    }

    #[test]
    fn media_request_requires_path_field() {
        let map = vec![(rmpv::Value::String("key".into()), rmpv::Value::Nil)];
        let mut buf = Vec::new();
        rmpv::encode::write_value(&mut buf, &rmpv::Value::Map(map)).unwrap();
        assert!(decode_media_request(&buf).is_err());
    }

    #[test]
    fn media_request_accepts_non_nil_key() {
        let map = vec![
            (
                rmpv::Value::String("path".into()),
                rmpv::Value::String("a.webp".into()),
            ),
            (
                rmpv::Value::String("key".into()),
                rmpv::Value::String("unused".into()),
            ),
        ];
        let mut buf = Vec::new();
        rmpv::encode::write_value(&mut buf, &rmpv::Value::Map(map)).unwrap();
        assert_eq!(decode_media_request(&buf).unwrap().path, "a.webp");
    }
}
