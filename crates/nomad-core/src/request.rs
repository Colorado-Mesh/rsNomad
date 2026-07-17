//! Decode NomadNet link request payloads (`field_*` / `var_*` MessagePack maps).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Parsed request fields from a Nomad page form submission.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct NomadRequestFields {
    /// Original MessagePack (or empty) bytes.
    #[serde(skip)]
    pub raw: Vec<u8>,
    /// String keys (`field_*`, `var_*`, etc.) mapped to string values.
    pub fields: BTreeMap<String, String>,
}

/// Decode request body bytes into typed fields.
///
/// Accepts a MessagePack map of string→string (canonical NomadNet form data).
/// Non-map or empty input yields an empty field map with `raw` preserved.
pub fn decode_request_fields(data: &[u8]) -> NomadRequestFields {
    let mut out = NomadRequestFields {
        raw: data.to_vec(),
        fields: BTreeMap::new(),
    };
    if data.is_empty() {
        return out;
    }
    let Ok(value) = rmpv::decode::read_value(&mut &*data) else {
        return out;
    };
    let rmpv::Value::Map(map) = value else {
        return out;
    };
    for (key, val) in map {
        let Some(k) = value_as_string(&key) else {
            continue;
        };
        let Some(v) = value_as_string(&val) else {
            continue;
        };
        out.fields.insert(k, v);
    }
    out
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
        let parsed = decode_request_fields(&buf);
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
        let parsed = decode_request_fields(b"not-msgpack");
        assert!(parsed.fields.is_empty());
        assert_eq!(parsed.raw, b"not-msgpack");
    }
}
