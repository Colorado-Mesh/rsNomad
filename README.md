<div align="center">

# rsNomad

**Rust Nomad Network page/file hosting for Reticulum.**

[![License: AGPL-3.0-or-later](https://img.shields.io/badge/license-AGPL--3.0--or--later-blue.svg)](LICENSE)
[![Rust 1.87+](https://img.shields.io/badge/rust-1.87%2B-orange.svg)](https://www.rust-lang.org)
[![Status](https://img.shields.io/badge/status-experimental-yellow.svg)](#feature-status)

[NomadNet](https://github.com/markqvist/NomadNet) |
[Reticulum Manual](https://reticulum.network/manual/) |
[rsReticulum](https://github.com/ratspeak/rsReticulum) |
[rsLXMF](https://github.com/ratspeak/rsLXMF) |
[mesh-client](https://github.com/Colorado-Mesh/mesh-client) |
[Ratspeak](https://github.com/ratspeak/Ratspeak)

</div>

---

rsNomad is a Rust implementation of Nomad Network **page, file, and media hosting**
over Reticulum Links, plus shared **client request helpers** (path hashes, bodies,
timeouts). This is not a fork of NomadNet; it is NomadNet interaction-layer
behavior written in a different language, focused on staying interoperable with
Python NomadNet **1.4.3** hosting (`Node.py` pages/files/media including
non-WebP→WebP conversion) and MeshChat / mesh-client browsers. It is not the
source-of-truth implementation — do not treat it as one.

Page hosting uses Reticulum Link request/response on aspect `nomadnetwork.node`.
It is **not** LXMF messaging; use [rsLXMF](https://github.com/ratspeak/rsLXMF) for
delivery and propagation. Browser UI, Micron rendering, and fetched-image LRU
caches belong in clients (see mesh-client); this crate is the embeddable
interaction layer.

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
// Optional: spawn() also ensures a default index from display_name.
store.write_page_rel("docs/help.mu", b"> Help\n")?;

let node = NomadNode::spawn(
    transport_tx,
    identity,
    store,
    NomadNodeConfig {
        display_name: "My Node".into(),
        announce_interval: Some(Duration::from_secs(3600)),
        announce_at_start: true,
        allow_executable_pages: false, // opt-in Unix CGI / executable .allowed
        // media_cache: MediaCacheConfig::memory_only() by default
        // For long-lived hosts: MediaCacheConfig::with_disk_root("/var/cache/nomad-media")
        ..NomadNodeConfig::default()
    },
)
.await?;

println!("serving at {}", node.destination_hash_hex());
node.store().write_page_rel("about.mu", b"> About\n")?;
node.reload_routes()?; // required after content CRUD so new routes are served
// Embedders: wipe conversion cache from any user "Clear cache" control:
node.clear_media_cache()?;
```

`NomadNode` registers the `nomadnetwork.node` destination, installs a Link
request handler for `/page/...`, `/file/...`, and `/media`, and announces with
the display name as raw UTF-8 app data (canonical NomadNet format). Form bodies
are decoded for CGI pages when `allow_executable_pages` is enabled; `/media`
uses `decode_media_request` (`path` + `key`). Non-WebP images under `pages/`
are converted to WebP (NomadNet 1.4.3) and stored in an **in-memory** conversion
cache by default.

**Client helpers** (no Link I/O): `build_page_request` / `build_file_request` /
`build_media_request`, `overall_timeout_secs`, `reply_file_name`, and related
constants — so sidecar/embedders do not reimplement path hashes or codecs.

This crate is not published to crates.io. For the full public API (CRUD helpers,
stats, announce, error types, limits), generate local docs:

```bash
cargo doc -p nomad-core --open
```

## Storage Layout

NomadNet-compatible roots:

```text
<base>/
|-- pages/
|   |-- index.mu
|   |-- index.mu.allowed          # optional identity ACL companion
|   |-- docs/help.mu
|   `-- header.webp               # in-page images via /media
`-- files/
    `-- manual.pdf
```

Mapping:

- `pages/index.mu` → `/page/index.mu`
- `pages/docs/help.mu` → `/page/docs/help.mu`
- `pages/header.webp` → `/media` request with `path` = `header.webp` (native WebP)
- `pages/photo.png` → `/media` request with `path` = `photo.png` (converted to WebP)
- `files/manual.pdf` → `/file/manual.pdf`

Paths are resolved under each root without following symlink components; `..`,
absolute escapes, NUL/backslash, and control characters are rejected. Default
size caps are **512 KiB** for pages and **32 MiB** for files/media.

**Media conversion cache:** NomadNet’s `converted_node` analogue. Default is
**in-memory LRU** (256 entries / 192 MiB). Disk is opt-in only via
`MediaCacheConfig::with_disk_root(path)` — the path must be **embedder-owned**,
never under `pages/` or `files/`. Call `NomadNode::clear_media_cache()` (and
optionally `clear_media_cache_key`) from any user-facing clear-cache control.
Do **not** implement a parallel ImageCache in the mesh-client reticulum sidecar
or write `<content_root>/cache/images/`.

**Trust model:** content directories are trusted local storage. Operators must
ensure they are not writable by untrusted local users. Symlink components are
rejected; hard links under the same volume are not rejected (a hard-linked file
inside the root is treated as ordinary content).

**ACL (`.allowed`):** a companion file `{resource}.allowed` next to a page,
file, or media path restricts access to listed identity hashes (32 hex chars
per line). Missing companion → allow. Paths ending in `.allowed` are never
served. Deny replies use Micron `not_allowed_page()` for pages/files; media
denies drop silently. Executable `.allowed` scripts run only when
`allow_executable_pages` is enabled (same sandbox as CGI); otherwise they are
read as static lists.

**CGI:** when `allow_executable_pages` is true (default **false**), Unix pages
with the execute bit are run as processes with a cleared environment
(`PATH` sanitized, `link_id` / `remote_identity` / `field_*` / `var_*`), ~10s
timeout, stdout capped to `max_page_bytes`, stderr discarded, no shell.
Windows never runs CGI. ACL is evaluated before CGI.

Missing `/page/...` routes return a Micron 404 body. Missing `/file/...` and
bad `/media` requests are dropped with no reply (NomadNet parity). Unknown path
hashes do **not** rescan the filesystem — call `reload_routes()` after content
CRUD.

## Protocol Notes

- Aspect: `nomadnetwork.node`
- Transport: Reticulum encrypted Link request/response (not LXMF)
- Wire path hash: first 16 bytes of SHA-256 of the exact path string
- Form data: `decode_request_fields` accepts a MessagePack map of string keys
  (e.g. `field_*`, `var_*`) with size/depth caps; wired into CGI env when
  executable pages are enabled
- Media: `encode_media_request` / `decode_media_request` for `{path, key}`
  maps (`key` may be Nil); route string exactly `/media`
- Media types: `.webp` served native; `.png` / `.jpg` / `.jpeg` / `.bmp` /
  `.gif` / `.tiff` converted to WebP (quality 85, max dimension 1200) with a
  clearable conversion cache (NomadNet 1.4.3 `Node.serve_media` parity)
- Large responses: use normal `Reply` bytes; `LinkManager` upgrades to a response
  Resource when the packed reply exceeds the Link MDU
- File / media responses: `ReplyFile` — a response Resource with raw bytes and
  msgpack metadata `{"name": ...}` (relative path for `/file`, basename for
  `/media`; converted media uses `*.webp` basename)
- Announce app data: raw UTF-8 display name, capped at 256 bytes (also accepted
  by mesh-client discovery)
- Hidden paths: dotfiles and `*.allowed` are not listed or served as content
  (NomadNet parity); `.allowed` companions are enforced as ACLs
- Concurrency: in-flight request budget (default 8) plus a fixed-window rate
  limit (default 60 requests / 10 s). The Link request handler runs
  synchronously on the link event loop with bounded disk reads.

## Feature Status

| Area | Current behavior |
| --- | --- |
| Static pages | Serve `.mu` (and other text) from `pages/` with 512 KiB default cap |
| Static files | Serve binaries from `files/` with 32 MiB default cap as response Resources with filename metadata |
| `/media` WebP | Exact `/media` route; native WebP + convert PNG/JPG/GIF/BMP/TIFF → WebP |
| Media conversion cache | In-memory LRU by default; optional embedder disk root; `clear_media_cache` / `clear_media_cache_key` |
| Client request helpers | `build_*_request`, timeout stages, `reply_file_name` (no Link I/O) |
| `.allowed` ACL | Static identity-hash lists; optional sandboxed executable companions |
| CGI pages | Opt-in (`allow_executable_pages`); Unix-only sandbox; default off |
| Announce | Startup + periodic + transport reannounce with display name |
| Form payload decode | Helpers + CGI env injection when enabled |
| Default index | Placeholder Micron page when `index.mu` is missing |
| Path safety | Traversal/symlink rejection, size limits, skip listing dotfiles/`*.allowed` |
| Request budget | Bounded in-flight handlers + fixed-window admit limit |
| Browser image/page LRU | mesh-client UI concern — not in this crate |
| Markdown CMS | Application concern (e.g. mesh-client UI) — not in this crate |
| Chat / forums | Roadmap only |
| `nomad-serve-rs` CLI | Planned (optional tools crate) |

## Compatibility Notes

Target clients: Python [NomadNet](https://github.com/markqvist/NomadNet) **1.4.3**
and MeshChat browsers, plus [mesh-client](https://github.com/Colorado-Mesh/mesh-client)
Nomad tab.

Compatibility target for hosting behavior is NomadNet **1.4.3**
(`Node.py`: `serve_page`, `serve_file`, `serve_media` with conversion,
`request_allowed`). CGI is **opt-in** and sandboxed (cleared env); Python
inherits the parent environment — an intentional hardening difference.

**Embedder contract (mesh-client reticulum sidecar):** call into `nomad-core`
only. Do not own a sidecar `ImageCache` or create `<content_root>/cache/images/`.
Wire any “Clear cache” UI to `NomadNode::clear_media_cache()`. Fetched remote
image caching stays in the TypeScript renderer.

This crate depends on Ratspeak [rsReticulum](https://github.com/ratspeak/rsReticulum)
path dependencies during development. It is not compatible with unrelated RNS
Rust stacks (for example TeskesLab `nomadnet-rs` / `rns-net`).

## Roadmap

Follow-ups (not required for basic hosting):

1. Optional `nomad-tools` binary (`nomad-serve-rs`) for headless static hosting
2. Richer Micron helpers / builders
3. Transfer repository ownership to the Ratspeak organization when permissions allow

Application-layer CMS, chat rooms, forums, LXMF image/file attachments, Micron
rendering, and **browser** image/page caches belong in clients such as
mesh-client / rsLXMF, not in this protocol crate.

## Contributing

Python NomadNet and Reticulum remain the reference implementations. Prefer
matching their on-wire behavior unless an intentional difference is documented.

Issues and pull requests are welcome on
[Colorado-Mesh/rsNomad](https://github.com/Colorado-Mesh/rsNomad).

## License

GNU Affero General Public License v3.0 or later. See [LICENSE](LICENSE).
