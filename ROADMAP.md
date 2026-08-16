# rsNomad Roadmap

This document parks follow-up work that is **out of scope** for the initial
static hosting release used by mesh-client (#613).

## Done (v0.1)

- Static `/page/...` and `/file/...` hosting over Reticulum Links
- `nomadnetwork.node` announce with UTF-8 display name
- Safe filesystem roots, size caps, Micron 404 / default index
- AGPL-3.0-or-later, Ratspeak-shaped README / CI
- MessagePack form encode/decode helpers (`encode_request_fields` /
  `decode_request_fields`) with shared size caps (decode not yet wired into
  the built-in serve handler)

## Near-term

- Optional `nomad-client` crate for shared fetch timeouts / Link query
  skeleton (mesh-client product policy such as `force_path_refresh` stays in
  clients)
- Optional `nomad-tools` crate with `nomad-serve-rs` headless binary
- Wire form/`field_*` bodies into serving when dynamic pages are designed
- Stronger interop fixtures against Python NomadNet page fetches
- Resource response filename metadata parity (may require rsReticulum upstream)
- Async / `spawn_blocking` serve path if LinkManager gains an async handler API

## Later (application / mesh-client)

These belong in clients such as mesh-client, not in the protocol crate:

- Markdown → Micron page composer / CMS workflow
- Theme and navigation editors
- NomadNet-style chat room apps
- Forums and other dynamic Nomad apps

## Explicit non-goals (v1)

- CGI / executable `.mu` page scripts (arbitrary code execution risk)
- Embedding hosting inside `rsLXMF`
- Depending on non-Ratspeak RNS stacks (`nomadnet-rs` / `rns-net`)

## Ownership

Repository currently: [Colorado-Mesh/rsNomad](https://github.com/Colorado-Mesh/rsNomad).
May transfer to the Ratspeak organization when permissions allow.
