//! Build and send `nomadnetwork.node` announces with UTF-8 display-name app data.

use bytes::Bytes;
use rns_identity::destination::Destination;
use rns_identity::identity::Identity;
use rns_transport::messages::{OutboundRequest, TransportMessage};
use tokio::sync::mpsc;

use crate::error::NomadError;
use crate::paths::NOMAD_NODE_ASPECT;

/// Max UTF-8 bytes included as announce / display-name app data.
pub const MAX_ANNOUNCE_NAME_BYTES: usize = 256;

/// Destination hash for `nomadnetwork.node` under `identity`.
pub fn nomad_destination_hash(identity: &Identity) -> [u8; 16] {
    Destination::hash_from_name_and_identity(NOMAD_NODE_ASPECT, Some(&identity.hash))
}

/// Trim, strip control characters, and UTF-8-safe truncate to [`MAX_ANNOUNCE_NAME_BYTES`].
pub fn clamp_node_name(name: &str) -> String {
    let mut name: String = name.trim().chars().filter(|c| !c.is_control()).collect();
    if name.len() > MAX_ANNOUNCE_NAME_BYTES {
        let mut end = MAX_ANNOUNCE_NAME_BYTES;
        while end > 0 && !name.is_char_boundary(end) {
            end -= 1;
        }
        name.truncate(end);
    }
    name
}

/// Build a raw announce packet with optional UTF-8 display name as app data.
pub fn build_nomad_announce_packet(
    identity: &Identity,
    display_name: Option<&str>,
) -> Result<Vec<u8>, NomadError> {
    let app_data_owned = display_name.map(clamp_node_name).filter(|s| !s.is_empty());
    let app_data = app_data_owned.as_ref().map(|s| s.as_bytes());
    let announce =
        rns_identity::announce::AnnounceData::create(identity, NOMAD_NODE_ASPECT, app_data, None)
            .map_err(|e| NomadError::message(e.to_string()))?;
    let dest_hash = nomad_destination_hash(identity);
    let flags = rns_wire::flags::PacketFlags {
        header_type: rns_wire::flags::HeaderType::Header1,
        context_flag: false,
        transport_type: rns_wire::flags::TransportType::Broadcast,
        destination_type: rns_wire::flags::DestinationType::Single,
        packet_type: rns_wire::flags::PacketType::Announce,
    };
    let header = rns_wire::header::PacketHeader {
        flags,
        hops: 0,
        transport_id: None,
        destination_hash: dest_hash,
        context: rns_wire::context::PacketContext::None,
    };
    let mut raw = header.pack();
    raw.extend_from_slice(&announce.pack());
    Ok(raw)
}

/// Queue an announce on the transport (non-blocking try_send).
pub fn send_nomad_announce_try(
    transport_tx: &mpsc::Sender<TransportMessage>,
    identity: &Identity,
    display_name: Option<&str>,
) {
    let raw = match build_nomad_announce_packet(identity, display_name) {
        Ok(raw) => raw,
        Err(e) => {
            tracing::warn!(error = %e, "failed to build nomad announce");
            return;
        }
    };
    let dest_hash = nomad_destination_hash(identity);
    if transport_tx
        .try_send(TransportMessage::Outbound(OutboundRequest {
            raw: Bytes::from(raw),
            destination_hash: dest_hash,
        }))
        .is_err()
    {
        tracing::debug!("nomad announce dropped (transport channel full)");
    }
}

/// Awaited announce send.
pub async fn send_nomad_announce(
    transport_tx: &mpsc::Sender<TransportMessage>,
    identity: &Identity,
    display_name: Option<&str>,
) -> Result<(), NomadError> {
    let raw = build_nomad_announce_packet(identity, display_name)?;
    let dest_hash = nomad_destination_hash(identity);
    transport_tx
        .send(TransportMessage::Outbound(OutboundRequest {
            raw: Bytes::from(raw),
            destination_hash: dest_hash,
        }))
        .await
        .map_err(|_| NomadError::TransportClosed)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn destination_hash_differs_from_identity_hash() {
        let identity = Identity::new();
        let dest = nomad_destination_hash(&identity);
        assert_ne!(dest, identity.hash);
    }

    #[test]
    fn announce_packet_nonempty() {
        let identity = Identity::new();
        let raw = build_nomad_announce_packet(&identity, Some("Demo Node")).unwrap();
        assert!(raw.len() > 64);
    }

    #[test]
    fn announce_name_is_length_capped() {
        let identity = Identity::new();
        let huge = "n".repeat(MAX_ANNOUNCE_NAME_BYTES + 100);
        let raw = build_nomad_announce_packet(&identity, Some(&huge)).unwrap();
        let short =
            build_nomad_announce_packet(&identity, Some(&"n".repeat(MAX_ANNOUNCE_NAME_BYTES)))
                .unwrap();
        assert_eq!(raw.len(), short.len());
    }

    #[test]
    fn announce_packet_none_and_empty_name() {
        let identity = Identity::new();
        let none = build_nomad_announce_packet(&identity, None).unwrap();
        let empty = build_nomad_announce_packet(&identity, Some("   ")).unwrap();
        assert_eq!(none.len(), empty.len());
        assert!(!none.is_empty());
    }

    #[test]
    fn clamp_trims_strips_controls_and_truncates_utf8() {
        assert_eq!(clamp_node_name("  hi\nthere  "), "hithere");
        let huge = format!("{}{}", "a".repeat(MAX_ANNOUNCE_NAME_BYTES - 1), "😀😀");
        let clamped = clamp_node_name(&huge);
        assert!(clamped.len() <= MAX_ANNOUNCE_NAME_BYTES);
        assert!(clamped.is_char_boundary(clamped.len()));
        assert!(clamped.starts_with('a'));
    }

    #[tokio::test]
    async fn send_nomad_announce_closed_channel() {
        let identity = Identity::new();
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        let err = send_nomad_announce(&tx, &identity, Some("x"))
            .await
            .unwrap_err();
        assert!(matches!(err, NomadError::TransportClosed));
    }
}
