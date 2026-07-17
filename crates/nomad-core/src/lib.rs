//! Nomad Network page/file hosting for Reticulum (`rsReticulum`).
//!
//! This crate implements the NomadNet application protocol over Reticulum Link
//! request/response (aspect `nomadnetwork.node`). It is not a fork of Python
//! NomadNet and is not the source-of-truth implementation.

mod announce;
mod error;
mod micron;
mod node;
mod paths;
mod request;
mod storage;

pub use announce::{build_nomad_announce_packet, nomad_destination_hash};
pub use error::NomadError;
pub use micron::{default_index_page, not_found_page};
pub use node::{NomadNode, NomadNodeConfig, NomadServeStats};
pub use paths::{
    NOMAD_NODE_ASPECT, normalize_file_route, normalize_page_route, path_hash, resolve_under_root,
    strip_page_prefix, validate_content_relative_path,
};
pub use request::{NomadRequestFields, decode_request_fields};
pub use storage::{NomadContentRoots, NomadContentStore, NomadPageEntry};
