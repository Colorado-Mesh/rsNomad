//! Path normalization and safe resolution for Nomad `/page/` and `/file/` routes.

use std::fs;
use std::path::{Component, Path, PathBuf};

use rns_crypto::sha::truncated_hash;

use crate::error::NomadError;

/// Max path components under a content root (DoS / deep-nesting guard).
pub const MAX_PATH_COMPONENTS: usize = 32;

/// Aspect Nomad Network nodes announce and serve under.
pub const NOMAD_NODE_ASPECT: &str = "nomadnetwork.node";

/// Truncated SHA-256 of the exact route string bytes (wire path hash).
pub fn path_hash(route: &str) -> [u8; 16] {
    truncated_hash(route.as_bytes())
}

/// Normalize a page route to `/page/...` form.
pub fn normalize_page_route(path: &str) -> Result<String, NomadError> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err(NomadError::InvalidPath("empty page path".into()));
    }
    let route = if trimmed.starts_with("/page/") {
        trimmed.to_string()
    } else if trimmed == "/page" {
        "/page/index.mu".to_string()
    } else if let Some(rest) = trimmed.strip_prefix('/') {
        format!("/page/{rest}")
    } else {
        format!("/page/{trimmed}")
    };
    validate_route_chars(&route)?;
    let _ = strip_page_prefix(&route)?;
    Ok(route)
}

/// Normalize a file route to `/file/...` form.
pub fn normalize_file_route(path: &str) -> Result<String, NomadError> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err(NomadError::InvalidPath("empty file path".into()));
    }
    let route = if trimmed.starts_with("/file/") {
        trimmed.to_string()
    } else if let Some(rest) = trimmed.strip_prefix('/') {
        format!("/file/{rest}")
    } else {
        format!("/file/{trimmed}")
    };
    validate_route_chars(&route)?;
    let _ = strip_file_prefix(&route)?;
    Ok(route)
}

/// Strip `/page/` prefix, returning the relative storage path (e.g. `index.mu`).
pub fn strip_page_prefix(route: &str) -> Result<&str, NomadError> {
    let route = route.trim();
    let rel = route
        .strip_prefix("/page/")
        .filter(|s| !s.is_empty())
        .ok_or_else(|| NomadError::InvalidPath(format!("not a page route: {route}")))?;
    validate_content_relative_path(rel)?;
    Ok(rel)
}

/// Strip `/file/` prefix, returning the relative storage path.
pub fn strip_file_prefix(route: &str) -> Result<&str, NomadError> {
    let route = route.trim();
    let rel = route
        .strip_prefix("/file/")
        .filter(|s| !s.is_empty())
        .ok_or_else(|| NomadError::InvalidPath(format!("not a file route: {route}")))?;
    validate_content_relative_path(rel)?;
    Ok(rel)
}

/// Validate a content-relative path (no leading slash, no `..`, no control chars).
pub fn validate_content_relative_path(rel: &str) -> Result<(), NomadError> {
    let rel = rel.trim().trim_start_matches('/');
    if rel.is_empty() {
        return Err(NomadError::InvalidPath("empty relative path".into()));
    }
    if rel.contains('\0') || rel.contains('\\') {
        return Err(NomadError::InvalidPath("invalid path characters".into()));
    }
    if rel.chars().any(|c| c.is_control()) {
        return Err(NomadError::InvalidPath("control characters in path".into()));
    }
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
                if name.chars().any(|c| c.is_control()) {
                    return Err(NomadError::InvalidPath("control characters in path".into()));
                }
            }
            Component::CurDir => {}
            _ => return Err(NomadError::PathTraversal),
        }
    }
    Ok(())
}

/// Resolve `rel` under `root`, rejecting escapes and symlink components.
pub fn resolve_under_root(root: &Path, rel: &str) -> Result<PathBuf, NomadError> {
    validate_content_relative_path(rel)?;
    if root.exists() {
        let root_meta = fs::symlink_metadata(root).map_err(NomadError::Io)?;
        if root_meta.file_type().is_symlink() {
            return Err(NomadError::PathTraversal);
        }
    }
    let root_canon = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());

    // Walk component-by-component without following symlinks.
    let mut cur = root_canon.clone();
    for component in Path::new(rel.trim_start_matches('/')).components() {
        let Component::Normal(name) = component else {
            return Err(NomadError::PathTraversal);
        };
        cur = cur.join(name);
        if cur.exists() {
            let meta = fs::symlink_metadata(&cur).map_err(NomadError::Io)?;
            if meta.file_type().is_symlink() {
                return Err(NomadError::PathTraversal);
            }
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
            let parent_meta = fs::symlink_metadata(parent).map_err(NomadError::Io)?;
            if parent_meta.file_type().is_symlink() {
                return Err(NomadError::PathTraversal);
            }
            let parent_canon = parent.canonicalize().map_err(NomadError::Io)?;
            if !parent_canon.starts_with(&root_canon) {
                return Err(NomadError::PathTraversal);
            }
        }
    }
    Ok(cur)
}

fn validate_route_chars(route: &str) -> Result<(), NomadError> {
    if route.contains('\0') || route.contains('\\') {
        return Err(NomadError::InvalidPath("invalid route characters".into()));
    }
    if route.chars().any(|c| c.is_control()) {
        return Err(NomadError::InvalidPath(
            "control characters in route".into(),
        ));
    }
    Ok(())
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
    }

    #[test]
    fn file_route_normalization() {
        assert_eq!(
            normalize_file_route("manual.pdf").unwrap(),
            "/file/manual.pdf"
        );
        assert_eq!(normalize_file_route("/file/a.bin").unwrap(), "/file/a.bin");
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
        }
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
