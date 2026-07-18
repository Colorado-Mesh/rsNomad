//! Path normalization and safe resolution for Nomad `/page/` and `/file/` routes.

use std::fs;
use std::path::{Component, Path, PathBuf};

use rns_crypto::sha::truncated_hash;

use crate::error::NomadError;

/// Max path components under a content root (DoS / deep-nesting guard).
pub const MAX_PATH_COMPONENTS: usize = 32;
/// Max UTF-8 bytes for a single path component name.
pub const MAX_COMPONENT_BYTES: usize = 255;
/// Max UTF-8 bytes for a full relative path.
pub const MAX_REL_PATH_BYTES: usize = 1024;

/// Wire prefix for page routes.
pub const PAGE_PREFIX: &str = "/page/";
/// Wire prefix for file routes.
pub const FILE_PREFIX: &str = "/file/";
/// Default page registered when the pages tree is empty.
pub const DEFAULT_INDEX_ROUTE: &str = "/page/index.mu";

/// Aspect Nomad Network nodes announce and serve under.
pub const NOMAD_NODE_ASPECT: &str = "nomadnetwork.node";

/// Truncated SHA-256 of the exact route string bytes (wire path hash).
pub fn path_hash(route: &str) -> [u8; 16] {
    truncated_hash(route.as_bytes())
}

/// Normalize a page route to `/page/...` form.
pub fn normalize_page_route(path: &str) -> Result<String, NomadError> {
    normalize_route(path, PAGE_PREFIX, Some("index.mu"), "page")
}

/// Normalize a file route to `/file/...` form.
///
/// Bare `/file` (no file name) is rejected — unlike `/page`, there is no
/// default file index.
pub fn normalize_file_route(path: &str) -> Result<String, NomadError> {
    normalize_route(path, FILE_PREFIX, None, "file")
}

fn normalize_route(
    path: &str,
    prefix: &str,
    default_index: Option<&str>,
    kind: &str,
) -> Result<String, NomadError> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err(NomadError::InvalidPath(format!("empty {kind} path")));
    }
    let bare = prefix.trim_end_matches('/');
    let route = if trimmed.starts_with(prefix) {
        trimmed.to_string()
    } else if trimmed == bare {
        match default_index {
            Some(index) => format!("{prefix}{index}"),
            None => {
                return Err(NomadError::InvalidPath(format!(
                    "bare {bare} route requires a {kind} name"
                )));
            }
        }
    } else if let Some(rest) = trimmed.strip_prefix('/') {
        format!("{prefix}{rest}")
    } else {
        format!("{prefix}{trimmed}")
    };
    validate_route_chars(&route)?;
    let _ = strip_route_prefix(&route, prefix, kind)?;
    Ok(route)
}

/// Strip `/page/` prefix, returning the relative storage path (e.g. `index.mu`).
pub fn strip_page_prefix(route: &str) -> Result<&str, NomadError> {
    strip_route_prefix(route, PAGE_PREFIX, "page")
}

/// Strip `/file/` prefix, returning the relative storage path.
pub fn strip_file_prefix(route: &str) -> Result<&str, NomadError> {
    strip_route_prefix(route, FILE_PREFIX, "file")
}

fn strip_route_prefix<'a>(route: &'a str, prefix: &str, kind: &str) -> Result<&'a str, NomadError> {
    let route = route.trim();
    let rel = route
        .strip_prefix(prefix)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| NomadError::InvalidPath(format!("not a {kind} route: {route}")))?;
    validate_content_relative_path(rel)?;
    Ok(rel)
}

/// True for NomadNet-style hidden / allowlist artifacts (dotfiles, `*.allowed`).
/// These must not be listed or served as content.
pub fn is_hidden_or_allowlist_name(name: &str) -> bool {
    let name = name.trim();
    if name.is_empty() {
        return false;
    }
    name.starts_with('.') || name.ends_with(".allowed")
}

/// Validate a content-relative path (no leading slash, no `..`, no control chars).
pub fn validate_content_relative_path(rel: &str) -> Result<(), NomadError> {
    let rel = rel.trim().trim_start_matches('/');
    if rel.is_empty() {
        return Err(NomadError::InvalidPath("empty relative path".into()));
    }
    if rel.len() > MAX_REL_PATH_BYTES {
        return Err(NomadError::InvalidPath("path too long".into()));
    }
    reject_invalid_chars(rel, "path")?;
    let path = Path::new(rel);
    let mut components = 0usize;
    for component in path.components() {
        match component {
            Component::Normal(name) => {
                components += 1;
                if components > MAX_PATH_COMPONENTS {
                    return Err(NomadError::InvalidPath("path too deep".into()));
                }
                let name = name.to_string_lossy();
                if name.len() > MAX_COMPONENT_BYTES {
                    return Err(NomadError::InvalidPath("path component too long".into()));
                }
                if name.chars().any(|c| c.is_control()) {
                    return Err(NomadError::InvalidPath("control characters in path".into()));
                }
                if is_hidden_or_allowlist_name(&name) {
                    return Err(NomadError::InvalidPath(
                        "hidden or allowlist paths are not served".into(),
                    ));
                }
            }
            // Reject `.` for consistent errors with resolve_under_root.
            Component::CurDir => return Err(NomadError::PathTraversal),
            _ => return Err(NomadError::PathTraversal),
        }
    }
    Ok(())
}

/// Resolve `rel` under `root`, rejecting escapes and symlink components.
pub fn resolve_under_root(root: &Path, rel: &str) -> Result<PathBuf, NomadError> {
    validate_content_relative_path(rel)?;
    if root.exists() {
        reject_if_symlink(root)?;
    }
    // Existing roots must canonicalize; missing roots (write-before-create) keep
    // the logical path and rely on component-wise symlink checks.
    let root_canon = if root.exists() {
        root.canonicalize().map_err(NomadError::Io)?
    } else {
        root.to_path_buf()
    };

    // Walk component-by-component without following symlinks.
    let mut cur = root_canon.clone();
    for component in Path::new(rel.trim_start_matches('/')).components() {
        let Component::Normal(name) = component else {
            return Err(NomadError::PathTraversal);
        };
        cur = cur.join(name);
        if cur.exists() {
            reject_if_symlink(&cur)?;
        }
    }

    if cur.exists() {
        let canon = cur.canonicalize().map_err(NomadError::Io)?;
        if !canon.starts_with(&root_canon) {
            return Err(NomadError::PathTraversal);
        }
        return Ok(canon);
    }
    if let Some(parent) = cur.parent() {
        if parent.exists() {
            reject_if_symlink(parent)?;
            let parent_canon = parent.canonicalize().map_err(NomadError::Io)?;
            if !parent_canon.starts_with(&root_canon) {
                return Err(NomadError::PathTraversal);
            }
        }
    }
    Ok(cur)
}

fn reject_if_symlink(path: &Path) -> Result<(), NomadError> {
    let meta = fs::symlink_metadata(path).map_err(NomadError::Io)?;
    if meta.file_type().is_symlink() {
        return Err(NomadError::PathTraversal);
    }
    Ok(())
}

fn reject_invalid_chars(s: &str, ctx: &str) -> Result<(), NomadError> {
    if s.contains('\0') || s.contains('\\') {
        return Err(NomadError::InvalidPath(format!("invalid {ctx} characters")));
    }
    if s.chars().any(|c| c.is_control()) {
        return Err(NomadError::InvalidPath(format!(
            "control characters in {ctx}"
        )));
    }
    Ok(())
}

fn validate_route_chars(route: &str) -> Result<(), NomadError> {
    reject_invalid_chars(route, "route")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn page_route_normalization() {
        assert_eq!(normalize_page_route("index.mu").unwrap(), "/page/index.mu");
        assert_eq!(
            normalize_page_route("/page/docs/help.mu").unwrap(),
            "/page/docs/help.mu"
        );
        assert_eq!(normalize_page_route("/page").unwrap(), DEFAULT_INDEX_ROUTE);
    }

    #[test]
    fn file_route_normalization() {
        assert_eq!(
            normalize_file_route("manual.pdf").unwrap(),
            "/file/manual.pdf"
        );
        assert_eq!(normalize_file_route("/file/a.bin").unwrap(), "/file/a.bin");
        assert!(matches!(
            normalize_file_route("/file"),
            Err(NomadError::InvalidPath(_))
        ));
    }

    #[test]
    fn rejects_traversal() {
        assert!(matches!(
            validate_content_relative_path("../secret"),
            Err(NomadError::PathTraversal)
        ));
        assert!(normalize_page_route("/page/../x.mu").is_err());
    }

    #[test]
    fn rejects_nul_backslash_and_curdir() {
        assert!(validate_content_relative_path("a\0b").is_err());
        assert!(validate_content_relative_path(r"a\b").is_err());
        // Leading slash is stripped by validation; CurDir is rejected.
        assert!(matches!(
            validate_content_relative_path("."),
            Err(NomadError::PathTraversal)
        ));
        assert!(matches!(
            validate_content_relative_path("./x.mu"),
            Err(NomadError::PathTraversal)
        ));
    }

    #[test]
    fn rejects_path_too_deep() {
        let deep = (0..=MAX_PATH_COMPONENTS)
            .map(|i| format!("d{i}"))
            .collect::<Vec<_>>()
            .join("/");
        let with_file = format!("{deep}/x.mu");
        assert!(matches!(
            validate_content_relative_path(&with_file),
            Err(NomadError::InvalidPath(_))
        ));
    }

    #[test]
    fn path_hash_is_truncated_sha256() {
        let h = path_hash("/page/index.mu");
        assert_eq!(h, truncated_hash(b"/page/index.mu"));
    }

    #[test]
    fn resolve_under_root_rejects_escape() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("pages");
        std::fs::create_dir_all(&root).unwrap();
        assert!(matches!(
            resolve_under_root(&root, "../escape.mu"),
            Err(NomadError::PathTraversal)
        ));
        let ok = resolve_under_root(&root, "index.mu").unwrap();
        assert!(ok.ends_with("index.mu"));
    }

    #[test]
    fn rejects_control_characters_and_symlink_components() {
        assert!(validate_content_relative_path("evil\nname.mu").is_err());
        let dir = tempdir().unwrap();
        let root = dir.path().join("pages");
        std::fs::create_dir_all(&root).unwrap();
        let outside = dir.path().join("secret.txt");
        std::fs::write(&outside, b"secret").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let link = root.join("link.mu");
            symlink(&outside, &link).unwrap();
            assert!(matches!(
                resolve_under_root(&root, "link.mu"),
                Err(NomadError::PathTraversal)
            ));

            // Intermediate directory symlink.
            let sub = root.join("sub");
            std::fs::create_dir_all(&sub).unwrap();
            let linked_dir = root.join("via");
            symlink(&sub, &linked_dir).unwrap();
            std::fs::write(sub.join("nested.mu"), b"x").unwrap();
            assert!(matches!(
                resolve_under_root(&root, "via/nested.mu"),
                Err(NomadError::PathTraversal)
            ));

            // Symlink root.
            let pages_link = dir.path().join("pages_link");
            symlink(&root, &pages_link).unwrap();
            assert!(matches!(
                resolve_under_root(&pages_link, "index.mu"),
                Err(NomadError::PathTraversal)
            ));
        }
    }

    #[test]
    fn rejects_dotfiles_and_allowed_suffix() {
        assert!(is_hidden_or_allowlist_name(".secret"));
        assert!(is_hidden_or_allowlist_name("index.mu.allowed"));
        assert!(!is_hidden_or_allowlist_name("index.mu"));
        assert!(validate_content_relative_path(".hidden.mu").is_err());
        assert!(validate_content_relative_path("docs/.keep").is_err());
        assert!(validate_content_relative_path("page.mu.allowed").is_err());
        assert!(normalize_page_route("/page/.hidden.mu").is_err());
        assert!(normalize_file_route("/file/notes.allowed").is_err());
    }
}

#[cfg(test)]
mod interop_tests {
    use super::*;

    /// Golden path hashes must match LinkClient::query / Python NomadNet truncated SHA-256.
    #[test]
    fn interop_path_hashes_match_wire_contract() {
        assert_eq!(
            hex::encode(path_hash("/page/index.mu")),
            hex::encode(truncated_hash(b"/page/index.mu"))
        );
        assert_eq!(
            hex::encode(path_hash("/file/readme.txt")),
            hex::encode(truncated_hash(b"/file/readme.txt"))
        );
        // Distinct routes must not collide for typical short names.
        assert_ne!(path_hash("/page/index.mu"), path_hash("/page/about.mu"));
    }
}
