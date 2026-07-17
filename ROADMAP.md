# rsNomad Roadmap

This document parks follow-up work that is **out of scope** for the initial
static hosting release used by mesh-client (#613).

## Done / in progress (v0.1)

- Static `/page/...` and `/file/...` hosting over Reticulum Links
- `nomadnetwork.node` announce with UTF-8 display name
- Safe filesystem roots, size caps, Micron 404 / default index
- MessagePack `field_*` / `var_*` request decode
- AGPL-3.0-or-later, Ratspeak-shaped README / CI

## Near-term

- Optional `nomad-tools` crate with `nomad-serve-rs` headless binary
- Stronger interop fixtures against Python NomadNet page fetches
- Resource response filename metadata parity (may require rsReticulum upstream)

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
