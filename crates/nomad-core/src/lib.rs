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

pub use announce::{MAX_ANNOUNCE_NAME_BYTES, build_nomad_announce_packet, nomad_destination_hash};
pub use error::NomadError;
pub use micron::{default_index_page, not_found_page, sanitize_micron_text};
pub use node::{NomadNode, NomadNodeConfig, NomadServeStats};
pub use paths::{
    DEFAULT_INDEX_ROUTE, FILE_PREFIX, MAX_PATH_COMPONENTS, NOMAD_NODE_ASPECT, PAGE_PREFIX,
    is_hidden_or_allowlist_name, normalize_file_route, normalize_page_route, path_hash,
    resolve_under_root, strip_file_prefix, strip_page_prefix, validate_content_relative_path,
};
pub use request::{
    MAX_REQUEST_BODY_BYTES, MAX_REQUEST_FIELDS, NomadRequestFields, decode_request_fields,
};
pub use storage::{
    DEFAULT_MAX_FILE_BYTES, DEFAULT_MAX_PAGE_BYTES, MAX_LISTED_ENTRIES, NomadContentRoots,
    NomadContentStore, NomadPageEntry,
};
