//! Filesystem content store for Nomad `pages/` and `files/` directories.

use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use crate::error::NomadError;
use crate::micron::default_index_page;
use crate::paths::{
    resolve_under_root, strip_file_prefix, strip_page_prefix, validate_content_relative_path,
};

/// Default max page body (matches mesh-client client limit).
pub const DEFAULT_MAX_PAGE_BYTES: usize = 512 * 1024;
/// Default max file body (matches mesh-client client limit).
pub const DEFAULT_MAX_FILE_BYTES: usize = 4 * 1024 * 1024;
/// Cap directory walk size to bound enumeration DoS.
pub const MAX_LISTED_ENTRIES: usize = 10_000;
/// Cap recursion depth when listing content.
const MAX_LIST_DEPTH: usize = 32;

#[derive(Debug, Clone)]
pub struct NomadContentRoots {
    pub pages_dir: PathBuf,
    pub files_dir: PathBuf,
    pub max_page_bytes: usize,
    pub max_file_bytes: usize,
}

impl NomadContentRoots {
    pub fn under(base: impl AsRef<Path>) -> Self {
        let base = base.as_ref();
        Self {
            pages_dir: base.join("pages"),
            files_dir: base.join("files"),
            max_page_bytes: DEFAULT_MAX_PAGE_BYTES,
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
        }
    }

    pub fn ensure_dirs(&self) -> Result<(), NomadError> {
        fs::create_dir_all(&self.pages_dir)?;
        fs::create_dir_all(&self.files_dir)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NomadPageEntry {
    pub path: String,
    pub size: u64,
    pub modified_ms: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct NomadContentStore {
    roots: NomadContentRoots,
}

impl NomadContentStore {
    pub fn new(roots: NomadContentRoots) -> Result<Self, NomadError> {
        roots.ensure_dirs()?;
        Ok(Self { roots })
    }

    pub fn roots(&self) -> &NomadContentRoots {
        &self.roots
    }

    /// Ensure `pages/index.mu` exists (writes a placeholder when missing).
    pub fn ensure_default_index(&self, display_name: &str) -> Result<(), NomadError> {
        let index = self.roots.pages_dir.join("index.mu");
        if !index.exists() {
            fs::write(&index, default_index_page(display_name))?;
        }
        Ok(())
    }

    pub fn list_pages(&self) -> Result<Vec<NomadPageEntry>, NomadError> {
        let mut out = Vec::new();
        collect_files(&self.roots.pages_dir, &self.roots.pages_dir, &mut out, 0)?;
        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(out)
    }

    pub fn list_files(&self) -> Result<Vec<NomadPageEntry>, NomadError> {
        let mut out = Vec::new();
        collect_files(&self.roots.files_dir, &self.roots.files_dir, &mut out, 0)?;
        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(out)
    }

    pub fn read_page_route(&self, route: &str) -> Result<Vec<u8>, NomadError> {
        let rel = strip_page_prefix(route)?;
        self.read_page_rel(rel)
    }

    pub fn read_page_rel(&self, rel: &str) -> Result<Vec<u8>, NomadError> {
        let path = resolve_under_root(&self.roots.pages_dir, rel)?;
        ensure_regular_file(&path, rel)?;
        read_limited(&path, self.roots.max_page_bytes)
    }

    pub fn write_page_rel(&self, rel: &str, content: &[u8]) -> Result<(), NomadError> {
        validate_content_relative_path(rel)?;
        if content.len() > self.roots.max_page_bytes {
            return Err(NomadError::TooLarge {
                size: content.len(),
                max: self.roots.max_page_bytes,
            });
        }
        let path = resolve_under_root(&self.roots.pages_dir, rel)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        atomic_write(&path, content)?;
        Ok(())
    }

    pub fn delete_page_rel(&self, rel: &str) -> Result<(), NomadError> {
        let path = resolve_under_root(&self.roots.pages_dir, rel)?;
        ensure_regular_file(&path, rel)?;
        fs::remove_file(path)?;
        Ok(())
    }

    pub fn read_file_route(&self, route: &str) -> Result<Vec<u8>, NomadError> {
        let rel = strip_file_prefix(route)?;
        self.read_file_rel(rel)
    }

    pub fn read_file_rel(&self, rel: &str) -> Result<Vec<u8>, NomadError> {
        let path = resolve_under_root(&self.roots.files_dir, rel)?;
        ensure_regular_file(&path, rel)?;
        read_limited(&path, self.roots.max_file_bytes)
    }

    pub fn write_file_rel(&self, rel: &str, content: &[u8]) -> Result<(), NomadError> {
        validate_content_relative_path(rel)?;
        if content.len() > self.roots.max_file_bytes {
            return Err(NomadError::TooLarge {
                size: content.len(),
                max: self.roots.max_file_bytes,
            });
        }
        let path = resolve_under_root(&self.roots.files_dir, rel)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        atomic_write(&path, content)?;
        Ok(())
    }

    pub fn delete_file_rel(&self, rel: &str) -> Result<(), NomadError> {
        let path = resolve_under_root(&self.roots.files_dir, rel)?;
        ensure_regular_file(&path, rel)?;
        fs::remove_file(path)?;
        Ok(())
    }
}

/// Read at most `max` bytes; reject without allocating the full oversize body.
fn read_limited(path: &Path, max: usize) -> Result<Vec<u8>, NomadError> {
    let file = File::open(path)?;
    let mut limited = file.take((max as u64).saturating_add(1));
    let mut buf = Vec::new();
    limited.read_to_end(&mut buf)?;
    if buf.len() > max {
        return Err(NomadError::TooLarge {
            size: buf.len(),
            max,
        });
    }
    Ok(buf)
}

fn ensure_regular_file(path: &Path, rel: &str) -> Result<(), NomadError> {
    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(NomadError::NotFound(rel.to_string()));
        }
        Err(e) => return Err(NomadError::Io(e)),
    };
    if meta.file_type().is_symlink() {
        return Err(NomadError::PathTraversal);
    }
    if !meta.is_file() {
        return Err(NomadError::NotFound(rel.to_string()));
    }
    Ok(())
}

fn atomic_write(path: &Path, content: &[u8]) -> Result<(), NomadError> {
    let parent = path
        .parent()
        .ok_or_else(|| NomadError::message("missing parent directory"))?;
    let file_name = path
        .file_name()
        .ok_or_else(|| NomadError::message("missing file name"))?
        .to_string_lossy();
    // Unique sibling temp name (not under os.tmpdir) so rename stays atomic on the same volume.
    let tmp_name = format!(
        ".{file_name}.{}.tmp",
        std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let tmp_path = parent.join(tmp_name);
    fs::write(&tmp_path, content)?;
    fs::rename(&tmp_path, path).map_err(|e| {
        let _ = fs::remove_file(&tmp_path);
        NomadError::Io(e)
    })?;
    Ok(())
}

fn collect_files(
    root: &Path,
    dir: &Path,
    out: &mut Vec<NomadPageEntry>,
    depth: usize,
) -> Result<(), NomadError> {
    if !dir.exists() {
        return Ok(());
    }
    if depth > MAX_LIST_DEPTH {
        return Err(NomadError::InvalidPath("directory tree too deep".into()));
    }
    let dir_meta = fs::symlink_metadata(dir).map_err(NomadError::Io)?;
    if dir_meta.file_type().is_symlink() {
        return Ok(());
    }
    for entry in fs::read_dir(dir)? {
        if out.len() >= MAX_LISTED_ENTRIES {
            return Err(NomadError::message("content listing exceeds limit"));
        }
        let entry = entry?;
        let path = entry.path();
        let meta = fs::symlink_metadata(&path)?;
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            collect_files(root, &path, out, depth + 1)?;
            continue;
        }
        if !meta.is_file() {
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .map_err(|_| NomadError::message("path outside content root"))?
            .to_string_lossy()
            .replace('\\', "/");
        let modified_ms = meta.modified().ok().and_then(|t| {
            t.duration_since(UNIX_EPOCH)
                .ok()
                .map(|d| d.as_millis() as u64)
        });
        out.push(NomadPageEntry {
            path: rel,
            size: meta.len(),
            modified_ms,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn page_crud_roundtrip() {
        let dir = tempdir().unwrap();
        let store = NomadContentStore::new(NomadContentRoots::under(dir.path())).unwrap();
        store.write_page_rel("index.mu", b"> Hello\n").unwrap();
        assert_eq!(store.read_page_rel("index.mu").unwrap(), b"> Hello\n");
        let pages = store.list_pages().unwrap();
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].path, "index.mu");
        store.delete_page_rel("index.mu").unwrap();
        assert!(store.read_page_rel("index.mu").is_err());
    }

    #[test]
    fn ensure_default_index() {
        let dir = tempdir().unwrap();
        let store = NomadContentStore::new(NomadContentRoots::under(dir.path())).unwrap();
        store.ensure_default_index("Demo").unwrap();
        let body = String::from_utf8(store.read_page_rel("index.mu").unwrap()).unwrap();
        assert!(body.contains("Demo"));
    }

    #[test]
    fn read_page_rejects_oversize_without_full_alloc_contract() {
        let dir = tempdir().unwrap();
        let mut roots = NomadContentRoots::under(dir.path());
        roots.max_page_bytes = 8;
        let store = NomadContentStore::new(roots).unwrap();
        store.write_page_rel("tiny.mu", b"ok").unwrap();
        // Bypass write guard by writing directly, then ensure read_limited rejects.
        std::fs::write(dir.path().join("pages/big.mu"), vec![b'x'; 32]).unwrap();
        let err = store.read_page_rel("big.mu").unwrap_err();
        assert!(matches!(err, NomadError::TooLarge { .. }));
    }
}
