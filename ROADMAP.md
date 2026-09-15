# rsNomad Roadmap

This document parks follow-up work that is **out of scope** for the initial
static hosting release used by mesh-client (#613).

## Done (v0.1)

- Static `/page/...` and `/file/...` hosting over Reticulum Links
- `nomadnetwork.node` announce with UTF-8 display name
- Safe filesystem roots, size caps, Micron 404 / default index
- AGPL-3.0-or-later, Ratspeak-shaped README / CI
- MessagePack form encode/decode helpers (`encode_request_fields` /
  `decode_request_fields`) with shared size caps
- `/file/...` response Resource filename metadata (`ReplyFile`, NomadNet
  `serve_file` parity); default file cap 32 MiB

## Done (NomadNet 1.4.1 PyPI target)

- `/media` WebP host (`encode_media_request` / `decode_media_request`,
  `ReplyFile` + basename metadata)
- `.allowed` identity ACL (static lists; optional sandboxed executable
  companions when CGI is enabled)
- Opt-in Unix CGI pages (`NomadNodeConfig.allow_executable_pages`, default off)
- Micron `not_allowed_page()` matching Python `DEFAULT_NOTALLOWED`

## Done (NomadNet 1.4.3 hosting + interaction helpers)

- Host `/media` conversion for PNG/JPG/JPEG/BMP/GIF/TIFF → WebP (NomadNet
  `Node.convert_media_to_webp` / `MEDIA_EXTS`)
- Clearable conversion cache: in-memory LRU default; optional embedder
  `MediaCacheConfig::disk_root`; `NomadNode::clear_media_cache` /
  `clear_media_cache_key` (never under content `pages/` / `files/`)
- Client Link request helpers: `build_page_request` / `build_file_request` /
  `build_media_request`, timeout stages, `reply_file_name`

## Near-term

- **Clients import existing `nomad-core` constants** (mesh-client sidecar still
  hardcodes some timeout helpers that this crate now exports — prefer
  `overall_timeout_secs` / `link_initiator_hops` over local duplicates).
- Optional `nomad-tools` crate with `nomad-serve-rs` headless binary
- Stronger interop fixtures against Python NomadNet page/media fetches
- Async / `spawn_blocking` serve path if LinkManager gains an async handler API

## Later (application / mesh-client)

These belong in clients such as mesh-client, not in the protocol crate:

- Browser fetched-image LRU / clear-cache UI (`browser/` analogue)
- Markdown → Micron page composer / CMS workflow
- Theme and navigation editors
- NomadNet-style chat room apps
- Forums and other dynamic Nomad apps
- LXMF conversation image/file attachments (rsLXMF + mesh-client UI)
- Nomad browser image preview for `/file/...` rasters and `/media` WebP

## Explicit non-goals

- Unsandboxed CGI with full parent-env inheritance (Python footgun; we clear env)
- Embedding hosting inside `rsLXMF`
- Depending on non-Ratspeak RNS stacks (`nomadnet-rs` / `rns-net`)
- Server-side MIME/`/image/` routes (in-page images use `/media`; other
  binaries remain ordinary `/file/...`)
- Sidecar-owned ImageCache or `<content_root>/cache/images/` trees
- TUI `converted_disp` terminal glyph cache

## Ownership

Repository currently: [Colorado-Mesh/rsNomad](https://github.com/Colorado-Mesh/rsNomad).
May transfer to the Ratspeak organization when permissions allow.
