//! Client-side Nomad Link request helpers (path hashes, bodies, timeouts).
//!
//! These do **not** open Links or render Micron — embedders (mesh-client sidecar)
//! still own transport. Helpers centralize wire identity so callers do not
//! reimplement path hashes / media codecs / default timeout stages.

use std::collections::BTreeMap;
use std::time::Duration;

use crate::error::NomadError;
use crate::paths::{
    FILE_PREFIX, MEDIA_ROUTE, PAGE_PREFIX, normalize_file_route, normalize_page_route, path_hash,
};
use crate::request::{encode_media_request, encode_request_fields};

/// MeshChat / NomadNet path lookup stage default.
pub const NOMAD_PATH_LOOKUP_SECS: u64 = 15;
/// MeshChat TCP link establishment default.
pub const NOMAD_TCP_LINK_ESTABLISH_SECS: u64 = 15;
/// Grace after path + link for TCP page transfer.
pub const NOMAD_TCP_TRANSFER_GRACE_SECS: u64 = 15;
/// Python RNS `DEFAULT_PER_HOP_TIMEOUT`.
pub const NOMAD_RF_PER_HOP_TIMEOUT_SECS: u64 = 6;
/// Python RNS first-hop component in link establishment.
pub const NOMAD_RF_FIRST_HOP_SECS: u64 = 6;
/// Extra grace for slow RF page transfers.
pub const NOMAD_RF_TRANSFER_GRACE_SECS: u64 = 30;
/// Cap overall RF budget.
pub const NOMAD_RF_MAX_OVERALL_SECS: u64 = 180;

/// Coarse egress class for timeout budgeting (product policy stays in clients).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NomadEgress {
    /// TCP / internet hub path.
    Tcp,
    /// LoRa / USB RNode RF.
    Rf,
    /// BLE RNode (same per-hop budget as RF).
    Ble,
    /// Unknown / generic network (treated like TCP stages).
    Network,
}

impl NomadEgress {
    /// Parse common interface class labels (`tcp`, `rf`, `ble`, `network`).
    pub fn parse(label: &str) -> Self {
        match label.trim().to_ascii_lowercase().as_str() {
            "rf" => Self::Rf,
            "ble" => Self::Ble,
            "tcp" => Self::Tcp,
            _ => Self::Network,
        }
    }
}

fn bounded_hops(hops: u8) -> u64 {
    u64::from(hops.clamp(1, 32))
}

/// Overall Link query deadline in seconds (path + establish + transfer grace).
pub fn overall_timeout_secs(egress: NomadEgress, hops: u8) -> u64 {
    match egress {
        NomadEgress::Rf | NomadEgress::Ble => {
            let bounded = bounded_hops(hops);
            let link_establish = NOMAD_RF_FIRST_HOP_SECS + NOMAD_RF_PER_HOP_TIMEOUT_SECS * bounded;
            let total = NOMAD_PATH_LOOKUP_SECS + link_establish + NOMAD_RF_TRANSFER_GRACE_SECS;
            total.min(NOMAD_RF_MAX_OVERALL_SECS)
        }
        NomadEgress::Tcp | NomadEgress::Network => {
            NOMAD_PATH_LOOKUP_SECS + NOMAD_TCP_LINK_ESTABLISH_SECS + NOMAD_TCP_TRANSFER_GRACE_SECS
        }
    }
}

/// [`overall_timeout_secs`] as a [`Duration`].
pub fn overall_timeout(egress: NomadEgress, hops: u8) -> Duration {
    Duration::from_secs(overall_timeout_secs(egress, hops))
}

/// Hops passed to a Link initiator (scales establishment timeout).
///
/// TCP/network: floor 3 / cap 7. RF/BLE: clamp 1..=32.
pub fn link_initiator_hops(egress: NomadEgress, path_hops: u8) -> u8 {
    match egress {
        NomadEgress::Rf | NomadEgress::Ble => path_hops.clamp(1, 32),
        NomadEgress::Tcp | NomadEgress::Network => path_hops.clamp(3, 7),
    }
}

/// Prepared Link REQUEST identity for a Nomad page/file/media fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NomadClientRequest {
    /// Wire path hash (first 16 bytes of SHA-256 of the exact route string).
    pub path_hash: [u8; 16],
    /// Exact route string (`/page/...`, `/file/...`, or `/media`).
    pub route: String,
    /// MessagePack request body (may be empty for page/file without fields).
    pub body: Vec<u8>,
}

/// Build a page Link request (`/page/...` + optional form fields).
pub fn build_page_request(
    path: &str,
    fields: Option<&BTreeMap<String, String>>,
) -> Result<NomadClientRequest, NomadError> {
    let route = if path.starts_with(PAGE_PREFIX) {
        normalize_page_route(path)?
    } else {
        normalize_page_route(&format!("{PAGE_PREFIX}{}", path.trim_start_matches('/')))?
    };
    let body = match fields {
        Some(f) if !f.is_empty() => encode_request_fields(f),
        _ => Vec::new(),
    };
    Ok(NomadClientRequest {
        path_hash: path_hash(&route),
        route,
        body,
    })
}

/// Build a file Link request (`/file/...`).
pub fn build_file_request(path: &str) -> Result<NomadClientRequest, NomadError> {
    let route = if path.starts_with(FILE_PREFIX) {
        normalize_file_route(path)?
    } else {
        normalize_file_route(&format!("{FILE_PREFIX}{}", path.trim_start_matches('/')))?
    };
    Ok(NomadClientRequest {
        path_hash: path_hash(&route),
        route,
        body: Vec::new(),
    })
}

/// Build a `/media` Link request body for `media_path` under `pages/`.
pub fn build_media_request(media_path: &str) -> NomadClientRequest {
    NomadClientRequest {
        path_hash: path_hash(MEDIA_ROUTE),
        route: MEDIA_ROUTE.to_string(),
        body: encode_media_request(media_path),
    }
}

/// Extract the `name` field from NomadNet file/media reply metadata (msgpack map).
pub fn reply_file_name(metadata: Option<&[u8]>) -> Option<String> {
    let data = metadata.filter(|d| !d.is_empty())?;
    let value = rmpv::decode::read_value(&mut &*data).ok()?;
    let map = value.as_map()?;
    for (k, v) in map {
        let key_ok = match k {
            rmpv::Value::String(s) => s.as_str() == Some("name"),
            _ => false,
        };
        if !key_ok {
            continue;
        }
        return match v {
            rmpv::Value::Binary(b) => String::from_utf8(b.clone()).ok(),
            rmpv::Value::String(s) => s.as_str().map(str::to_owned),
            _ => None,
        };
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request::decode_media_request;

    #[test]
    fn tcp_timeout_is_45s() {
        assert_eq!(overall_timeout_secs(NomadEgress::Tcp, 8), 45);
        assert_eq!(overall_timeout_secs(NomadEgress::Network, 1), 45);
    }

    #[test]
    fn rf_timeout_scales() {
        assert_eq!(overall_timeout_secs(NomadEgress::Rf, 1), 57);
        assert_eq!(overall_timeout_secs(NomadEgress::Rf, 32), 180);
        assert_eq!(overall_timeout_secs(NomadEgress::Ble, 6), 87);
    }

    #[test]
    fn initiator_hops_floored_for_tcp() {
        assert_eq!(link_initiator_hops(NomadEgress::Tcp, 1), 3);
        assert_eq!(link_initiator_hops(NomadEgress::Tcp, 8), 7);
        assert_eq!(link_initiator_hops(NomadEgress::Rf, 8), 8);
    }

    #[test]
    fn page_request_hashes_normalized_route() {
        let req = build_page_request("index.mu", None).unwrap();
        assert_eq!(req.route, "/page/index.mu");
        assert_eq!(req.path_hash, path_hash("/page/index.mu"));
        assert!(req.body.is_empty());
    }

    #[test]
    fn page_request_with_fields() {
        let mut fields = BTreeMap::new();
        fields.insert("field_q".into(), "hi".into());
        let req = build_page_request("/page/search.mu", Some(&fields)).unwrap();
        assert!(!req.body.is_empty());
    }

    #[test]
    fn media_request_round_trip() {
        let req = build_media_request("header.webp");
        assert_eq!(req.route, MEDIA_ROUTE);
        assert_eq!(req.path_hash, path_hash(MEDIA_ROUTE));
        let parsed = decode_media_request(&req.body).unwrap();
        assert_eq!(parsed.path, "header.webp");
    }

    #[test]
    fn reply_file_name_from_binary_metadata() {
        let meta = rns_runtime::link_manager::pack_file_name_metadata("Hero.webp");
        assert_eq!(reply_file_name(Some(&meta)).as_deref(), Some("Hero.webp"));
        assert_eq!(reply_file_name(None), None);
    }
}
