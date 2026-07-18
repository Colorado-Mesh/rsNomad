//! Decode NomadNet link request payloads (`field_*` / `var_*` MessagePack maps).
//!
//! This helper is available for callers that want to interpret form bodies.
//! The built-in [`crate::NomadNode`] request handler serves static content only
//! and currently ignores the request body.

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
