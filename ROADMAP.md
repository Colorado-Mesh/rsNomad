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

## Near-term

- **Clients import existing `nomad-core` constants** (mesh-client sidecar still
  hardcodes `NOMAD_NODE_ASPECT` and page/file size caps that this crate already
  exports). No rsNomad change required — switch the sidecar to
  `NOMAD_NODE_ASPECT`, `DEFAULT_MAX_PAGE_BYTES`, and `DEFAULT_MAX_FILE_BYTES`.
- Optional `nomad-client` crate for shared fetch timeouts / Link query
  skeleton — add when a **second Rust consumer** appears (e.g. `nomad-tools`)
  or timeout constants drift and cause bugs. Until then keep timeout math in
  the sidecar; mesh-client product policy such as `force_path_refresh` stays in
  clients. TS UI/proxy mirrors remain client-side.
- Optional `nomad-tools` crate with `nomad-serve-rs` headless binary
- Stronger interop fixtures against Python NomadNet page fetches
- Async / `spawn_blocking` serve path if LinkManager gains an async handler API

## Later (application / mesh-client)

These belong in clients such as mesh-client, not in the protocol crate:

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
- Server-side MIME/`/image/` routes (in-page images use `/media` WebP; other
  binaries remain ordinary `/file/...`)

## Ownership

Repository currently: [Colorado-Mesh/rsNomad](https://github.com/Colorado-Mesh/rsNomad).
May transfer to the Ratspeak organization when permissions allow.
