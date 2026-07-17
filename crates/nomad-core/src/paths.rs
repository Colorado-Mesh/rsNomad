//! Path normalization and safe resolution for Nomad `/page/` and `/file/` routes.

use std::path::{Component, Path, PathBuf};

use rns_crypto::sha::truncated_hash;

use crate::error::NomadError;

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

/// Validate a content-relative path (no leading slash, no `..`).
pub fn validate_content_relative_path(rel: &str) -> Result<(), NomadError> {
    let rel = rel.trim().trim_start_matches('/');
    if rel.is_empty() {
        return Err(NomadError::InvalidPath("empty relative path".into()));
    }
    if rel.contains('\0') || rel.contains('\\') {
        return Err(NomadError::InvalidPath("invalid path characters".into()));
    }
    let path = Path::new(rel);
    for component in path.components() {
        match component {
            Component::Normal(_) => {}
            Component::CurDir => {}
            _ => return Err(NomadError::PathTraversal),
        }
    }
    Ok(())
}

/// Resolve `rel` under `root`, rejecting escapes outside the root.
pub fn resolve_under_root(root: &Path, rel: &str) -> Result<PathBuf, NomadError> {
    validate_content_relative_path(rel)?;
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let candidate = root.join(rel.trim_start_matches('/'));
    if let Ok(canon) = candidate.canonicalize() {
        if !canon.starts_with(&root) {
            return Err(NomadError::PathTraversal);
        }
        return Ok(canon);
    }
    if let Some(parent) = candidate.parent() {
        if parent.exists() {
            let parent_canon = parent.canonicalize().map_err(NomadError::Io)?;
            if !parent_canon.starts_with(&root) {
                return Err(NomadError::PathTraversal);
            }
        }
    }
    Ok(candidate)
}

fn validate_route_chars(route: &str) -> Result<(), NomadError> {
    if route.contains('\0') || route.contains('\\') {
        return Err(NomadError::InvalidPath("invalid route characters".into()));
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
}
