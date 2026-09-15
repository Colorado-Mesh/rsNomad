//! Nomad Network page/file hosting for Reticulum (`rsReticulum`).
//!
//! This crate implements the NomadNet application protocol over Reticulum Link
//! request/response (aspect `nomadnetwork.node`). It is not a fork of Python
//! NomadNet and is not the source-of-truth implementation.

mod acl;
mod announce;
mod cgi;
mod client;
mod error;
mod media_cache;
mod media_convert;
mod micron;
mod node;
mod paths;
mod request;
mod storage;

pub use announce::{
    MAX_ANNOUNCE_NAME_BYTES, build_nomad_announce_packet, clamp_node_name, nomad_destination_hash,
};
pub use client::{
    NOMAD_PATH_LOOKUP_SECS, NOMAD_RF_FIRST_HOP_SECS, NOMAD_RF_MAX_OVERALL_SECS,
    NOMAD_RF_PER_HOP_TIMEOUT_SECS, NOMAD_RF_TRANSFER_GRACE_SECS, NOMAD_TCP_LINK_ESTABLISH_SECS,
    NOMAD_TCP_TRANSFER_GRACE_SECS, NomadClientRequest, NomadEgress, build_file_request,
    build_media_request, build_page_request, link_initiator_hops, overall_timeout,
    overall_timeout_secs, reply_file_name,
};
pub use error::NomadError;
pub use media_cache::{
    DEFAULT_MEDIA_CACHE_MAX_BYTES, DEFAULT_MEDIA_CACHE_MAX_ENTRIES, MediaCache, MediaCacheConfig,
    conversion_cache_key,
};
pub use media_convert::{
    DEFAULT_CONVERSION_MAX_DIMENSION, DEFAULT_CONVERSION_QUALITY, MEDIA_EXTS, NATIVE_MEDIA_EXTS,
    cache_key_for_source, convert_bytes_to_webp, converted_basename, is_media_ext,
    is_native_media_ext, media_extension, source_content_sha256_hex,
};
pub use micron::{
    MAX_MICRON_TEXT_CHARS, default_index_page, not_allowed_page, not_found_page,
    sanitize_micron_text,
};
pub use node::{NomadNode, NomadNodeConfig, NomadServeStats};
pub use paths::{
    DEFAULT_INDEX_ROUTE, FILE_PREFIX, MAX_COMPONENT_BYTES, MAX_PATH_COMPONENTS, MAX_REL_PATH_BYTES,
    MEDIA_ROUTE, NOMAD_NODE_ASPECT, PAGE_PREFIX, is_hidden_or_allowlist_name, normalize_file_route,
    normalize_page_route, path_hash, resolve_under_root, strip_file_prefix, strip_page_prefix,
    validate_content_relative_path,
};
pub use request::{
    MAX_REQUEST_BODY_BYTES, MAX_REQUEST_FIELD_KEY_BYTES, MAX_REQUEST_FIELD_VALUE_BYTES,
    MAX_REQUEST_FIELDS, MAX_REQUEST_MSGPACK_DEPTH, MediaRequest, NomadRequestFields,
    decode_media_request, decode_request_fields, encode_media_request, encode_request_fields,
};
pub use storage::{
    DEFAULT_MAX_FILE_BYTES, DEFAULT_MAX_PAGE_BYTES, MAX_LISTED_ENTRIES, NomadContentRoots,
    NomadContentStore, NomadPageEntry,
};
