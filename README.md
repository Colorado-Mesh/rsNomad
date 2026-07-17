<div align="center">

# rsNomad

**Rust Nomad Network page/file hosting for Reticulum.**

[![License: AGPL-3.0-or-later](https://img.shields.io/badge/license-AGPL--3.0--or--later-blue.svg)](LICENSE)
[![Rust 1.85+](https://img.shields.io/badge/rust-1.85%2B-orange.svg)](https://www.rust-lang.org)
[![Status](https://img.shields.io/badge/status-experimental-yellow.svg)](#feature-status)

[NomadNet](https://github.com/markqvist/NomadNet) |
[Reticulum Manual](https://reticulum.network/manual/) |
[rsReticulum](https://github.com/ratspeak/rsReticulum) |
[rsLXMF](https://github.com/ratspeak/rsLXMF) |
[mesh-client](https://github.com/Colorado-Mesh/mesh-client) |
[Ratspeak](https://github.com/ratspeak/Ratspeak)

</div>

---

rsNomad is a Rust implementation of Nomad Network **static page and file hosting**
over Reticulum Links. This is not a fork of NomadNet; it is NomadNet page-server
behavior written in a different language, focused on staying interoperable with
Python NomadNet and MeshChat. It is not the source-of-truth implementation — do
not treat it as one.

Page hosting uses Reticulum Link request/response on aspect `nomadnetwork.node`.
It is **not** LXMF messaging; use [rsLXMF](https://github.com/ratspeak/rsLXMF) for
delivery and propagation.

This repository currently lives under
[Colorado-Mesh/rsNomad](https://github.com/Colorado-Mesh/rsNomad). Layout, license,
and CI match the Ratspeak sibling crates so the project can move to the Ratspeak
organization later with minimal churn.

## Contents

- [Build It](#build-it)
- [Library Usage](#library-usage)
- [Storage Layout](#storage-layout)
- [Protocol Notes](#protocol-notes)
- [Feature Status](#feature-status)
- [Compatibility Notes](#compatibility-notes)
- [Roadmap](#roadmap)
- [Contributing](#contributing)
- [License](#license)

## Build It

Development requires [rsReticulum](https://github.com/ratspeak/rsReticulum) as a
sibling directory next to this repo:

```text
colorado-mesh-src/   # or ratspeak-src/ later
|-- rsReticulum/     # from ratspeak/rsReticulum
|-- rsLXMF/          # optional; not required for rsNomad core
`-- rsNomad/
```

If you are starting fresh:

```bash
mkdir colorado-mesh-src
cd colorado-mesh-src
git clone https://github.com/ratspeak/rsReticulum
git clone https://github.com/Colorado-Mesh/rsNomad
cd rsNomad
```

### macOS

Install Rust with `rustup`, then install Apple's build tools:

```bash
xcode-select --install
```

```bash
cd rsNomad
cargo build --release
cargo test --workspace
```

### Linux / Raspberry Pi

Debian, Ubuntu, and Raspberry Pi OS:

```bash
sudo apt update
sudo apt install -y build-essential pkg-config
```

Fedora:

```bash
sudo dnf install gcc make pkgconf-pkg-config
```

Arch:

```bash
sudo pacman -S --needed base-devel pkgconf
```

```bash
cd rsNomad
cargo build --release
cargo test --workspace
```

## Library Usage

```rust
use nomad_core::{
    NomadContentRoots, NomadContentStore, NomadNode, NomadNodeConfig,
};
use std::time::Duration;

// Given a live rsReticulum transport channel + identity:
let store = NomadContentStore::new(NomadContentRoots::under("/path/to/nomadnetwork"))?;
store.ensure_default_index("My Node")?;

let node = NomadNode::spawn(
    transport_tx,
    identity,
    store,
    NomadNodeConfig {
        display_name: "My Node".into(),
        announce_interval: Some(Duration::from_secs(3600)),
        announce_at_start: true,
    },
)
.await?;

println!("serving at {}", node.destination_hash_hex());
node.reload_routes()?; // after writing pages/files on disk
```

`NomadNode` registers the `nomadnetwork.node` destination, installs a Link
request handler for `/page/...` and `/file/...`, and announces with the display
name as raw UTF-8 app data (canonical NomadNet format).

## Storage Layout

NomadNet-compatible roots:

```text
<base>/
|-- pages/
|   |-- index.mu
|   `-- docs/help.mu
`-- files/
    `-- manual.pdf
```

Mapping:

- `pages/index.mu` → `/page/index.mu`
- `pages/docs/help.mu` → `/page/docs/help.mu`
- `files/manual.pdf` → `/file/manual.pdf`

Paths are canonicalized under each root; `..` and absolute escapes are rejected.

## Protocol Notes

- Aspect: `nomadnetwork.node`
- Transport: Reticulum encrypted Link request/response (not LXMF)
- Wire path hash: first 16 bytes of SHA-256 of the exact path string
- Form data: MessagePack map of string keys (`field_*`, `var_*`) in the request body
- Large responses: use normal `Reply` bytes; `LinkManager` upgrades to a response
  Resource when the packed reply exceeds the Link MDU
- Announce app data: raw UTF-8 display name (also accepted by mesh-client discovery)

## Feature Status

| Area | Current behavior |
| --- | --- |
| Static pages | Serve `.mu` (and other text) from `pages/` with size caps |
| Static files | Serve binaries from `files/` with size caps |
| Announce | Startup + periodic + transport reannounce with display name |
| Form payload decode | MessagePack `field_*` / `var_*` maps |
| Default index | Placeholder Micron page when `index.mu` is missing |
| Path safety | Traversal rejection, response size limits |
| CGI / executable pages | **Not implemented** (explicit non-goal for v1) |
| Markdown CMS | Application concern (e.g. mesh-client UI) — not in this crate |
| Chat / forums | Roadmap only |
| `nomad-serve-rs` CLI | Planned (optional tools crate) |

## Compatibility Notes

Target clients: Python [NomadNet](https://github.com/markqvist/NomadNet) and MeshChat
browsers, plus [mesh-client](https://github.com/Colorado-Mesh/mesh-client) Nomad tab.

v1 focuses on static hosting. Dynamic executable pages (NomadNet CGI-style
`.mu` scripts) are intentionally omitted for security.

This crate depends on Ratspeak [rsReticulum](https://github.com/ratspeak/rsReticulum)
path dependencies during development. It is not compatible with unrelated RNS
Rust stacks (for example TeskesLab `nomadnet-rs` / `rns-net`).

## Roadmap

Follow-ups (not required for basic hosting):

1. Optional `nomad-tools` binary (`nomad-serve-rs`) for headless static hosting
2. Identity-restricted pages (`.mu.allowed` lists) without process execution
3. Richer Micron helpers / builders
4. Upstream Resource filename metadata improvements in rsReticulum if needed
5. Transfer repository ownership to the Ratspeak organization when permissions allow

Application-layer CMS, chat rooms, and forums belong in clients such as
mesh-client, not in this protocol crate.

## Contributing

Python NomadNet and Reticulum remain the reference implementations. Prefer
matching their on-wire behavior unless an intentional difference is documented.

Issues and pull requests are welcome on
[Colorado-Mesh/rsNomad](https://github.com/Colorado-Mesh/rsNomad).

## License

GNU Affero General Public License v3.0 or later. See [LICENSE](LICENSE).
