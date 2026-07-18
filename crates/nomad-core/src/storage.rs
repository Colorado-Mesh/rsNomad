//! Filesystem content store for Nomad `pages/` and `files/` directories.
//!
//! Content directories are trusted local storage: operators must ensure they are
//! not writable by untrusted local users. Symlink components are rejected;
//! hard links under the same volume are not rejected (document the trust model).

use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use crate::error::NomadError;
use crate::micron::default_index_page;
use crate::paths::{
    is_hidden_or_allowlist_name, resolve_under_root, strip_file_prefix, strip_page_prefix,
    validate_content_relative_path,
};

/// Default max page body (matches mesh-client client limit).
pub const DEFAULT_MAX_PAGE_BYTES: usize = 512 * 1024;
/// Default max file body (matches mesh-client client limit).
pub const DEFAULT_MAX_FILE_BYTES: usize = 4 * 1024 * 1024;
/// Cap directory walk size to bound enumeration DoS.
pub const MAX_LISTED_ENTRIES: usize = 10_000;
/// Cap recursion depth when listing content.
const MAX_LIST_DEPTH: usize = 32;

/// Local filesystem roots and size caps for Nomad content.
#[derive(Debug, Clone)]
pub struct NomadContentRoots {
    /// Directory served as `/page/...` routes.
    pub pages_dir: PathBuf,
    /// Directory served as `/file/...` routes.
    pub files_dir: PathBuf,
    /// Max bytes read/written for a page body.
    pub max_page_bytes: usize,
    /// Max bytes read/written for a file body.
    pub max_file_bytes: usize,
}

impl NomadContentRoots {
    /// Build roots as `<base>/pages` and `<base>/files` with default size caps.
    pub fn under(base: impl AsRef<Path>) -> Self {
        let base = base.as_ref();
        Self {
            pages_dir: base.join("pages"),
            files_dir: base.join("files"),
            max_page_bytes: DEFAULT_MAX_PAGE_BYTES,
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
        }
    }

    /// Create both content directories if missing.
    pub fn ensure_dirs(&self) -> Result<(), NomadError> {
        fs::create_dir_all(&self.pages_dir)?;
        fs::create_dir_all(&self.files_dir)?;
        Ok(())
    }
}

/// Listed page or file metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NomadPageEntry {
    /// Path relative to the content root (forward slashes).
    pub path: String,
    /// File size in bytes.
    pub size: u64,
    /// Last-modified time as Unix milliseconds, when available.
    pub modified_ms: Option<u64>,
}

/// Read/write/list pages and files under configured content roots.
#[derive(Debug, Clone)]
pub struct NomadContentStore {
    roots: NomadContentRoots,
}

impl NomadContentStore {
    /// Create the store, ensuring content directories exist.
    pub fn new(roots: NomadContentRoots) -> Result<Self, NomadError> {
        roots.ensure_dirs()?;
        Ok(Self { roots })
    }

    /// Borrow the configured roots and size caps.
    pub fn roots(&self) -> &NomadContentRoots {
        &self.roots
    }

    /// Ensure `pages/index.mu` exists (writes a placeholder when missing).
    ///
    /// Uses [`resolve_under_root`] and atomic write so a dangling symlink at
    /// `index.mu` cannot redirect the write outside the content tree.
    pub fn ensure_default_index(&self, display_name: &str) -> Result<(), NomadError> {
        let path = resolve_under_root(&self.roots.pages_dir, "index.mu")?;
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_symlink() => return Err(NomadError::PathTraversal),
            Ok(_) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(NomadError::Io(e)),
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        atomic_write(&path, default_index_page(display_name).as_bytes())?;
        Ok(())
    }

    /// List pages under `pages/` (skips dotfiles / `*.allowed`).
    pub fn list_pages(&self) -> Result<Vec<NomadPageEntry>, NomadError> {
        list_under(&self.roots.pages_dir)
    }

    /// List files under `files/` (skips dotfiles / `*.allowed`).
    pub fn list_files(&self) -> Result<Vec<NomadPageEntry>, NomadError> {
        list_under(&self.roots.files_dir)
    }

    /// Read a page by wire route (`/page/...`).
    pub fn read_page_route(&self, route: &str) -> Result<Vec<u8>, NomadError> {
        let rel = strip_page_prefix(route)?;
        self.read_page_rel(rel)
    }

    /// Read a page by content-relative path.
    pub fn read_page_rel(&self, rel: &str) -> Result<Vec<u8>, NomadError> {
        read_rel(&self.roots.pages_dir, self.roots.max_page_bytes, rel)
    }

    /// Atomically write a page by content-relative path.
    pub fn write_page_rel(&self, rel: &str, content: &[u8]) -> Result<(), NomadError> {
        write_rel(
            &self.roots.pages_dir,
            self.roots.max_page_bytes,
            rel,
            content,
        )
    }

    /// Delete a page by content-relative path.
    pub fn delete_page_rel(&self, rel: &str) -> Result<(), NomadError> {
        delete_rel(&self.roots.pages_dir, rel)
    }

    /// Read a file by wire route (`/file/...`).
    pub fn read_file_route(&self, route: &str) -> Result<Vec<u8>, NomadError> {
        let rel = strip_file_prefix(route)?;
        self.read_file_rel(rel)
    }

    /// Read a file by content-relative path.
    pub fn read_file_rel(&self, rel: &str) -> Result<Vec<u8>, NomadError> {
        read_rel(&self.roots.files_dir, self.roots.max_file_bytes, rel)
    }

    /// Atomically write a file by content-relative path.
    pub fn write_file_rel(&self, rel: &str, content: &[u8]) -> Result<(), NomadError> {
        write_rel(
            &self.roots.files_dir,
            self.roots.max_file_bytes,
            rel,
            content,
        )
    }

    /// Delete a file by content-relative path.
    pub fn delete_file_rel(&self, rel: &str) -> Result<(), NomadError> {
        delete_rel(&self.roots.files_dir, rel)
    }
}

fn list_under(root: &Path) -> Result<Vec<NomadPageEntry>, NomadError> {
    let mut out = Vec::new();
    collect_files(root, root, &mut out, 0)?;
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

fn read_rel(root: &Path, max: usize, rel: &str) -> Result<Vec<u8>, NomadError> {
    let path = resolve_under_root(root, rel)?;
    ensure_regular_file(&path, rel)?;
    read_limited(&path, max)
}

fn write_rel(root: &Path, max: usize, rel: &str, content: &[u8]) -> Result<(), NomadError> {
    validate_content_relative_path(rel)?;
    if content.len() > max {
        return Err(NomadError::TooLarge {
            size: content.len(),
            max,
        });
    }
    let path = resolve_under_root(root, rel)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    atomic_write(&path, content)?;
    Ok(())
}

fn delete_rel(root: &Path, rel: &str) -> Result<(), NomadError> {
    let path = resolve_under_root(root, rel)?;
    ensure_regular_file(&path, rel)?;
    fs::remove_file(path)?;
    Ok(())
}

/// Read at most `max` bytes; reject without allocating the full oversize body.
fn read_limited(path: &Path, max: usize) -> Result<Vec<u8>, NomadError> {
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        return Err(NomadError::PathTraversal);
    }
    if meta.len() > max as u64 {
        return Err(NomadError::TooLarge {
            size: usize::try_from(meta.len()).unwrap_or(usize::MAX),
            max,
        });
    }
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
        let file_name = entry.file_name().to_string_lossy().into_owned();
        // NomadNet parity: never list/serve dotfiles or `*.allowed` allowlists.
        if is_hidden_or_allowlist_name(&file_name) {
            continue;
        }
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
        if validate_content_relative_path(&rel).is_err() {
            continue;
        }
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
    use tempfile::{TempDir, tempdir};

    fn test_store() -> (TempDir, NomadContentStore) {
        let dir = tempdir().unwrap();
        let store = NomadContentStore::new(NomadContentRoots::under(dir.path())).unwrap();
        (dir, store)
    }

    #[test]
    fn page_crud_roundtrip() {
        let (_dir, store) = test_store();
        store.write_page_rel("index.mu", b"> Hello\n").unwrap();
        assert_eq!(store.read_page_rel("index.mu").unwrap(), b"> Hello\n");
        let pages = store.list_pages().unwrap();
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].path, "index.mu");
        store.delete_page_rel("index.mu").unwrap();
        assert!(store.read_page_rel("index.mu").is_err());
    }

    #[test]
    fn file_crud_and_delete_missing() {
        let (_dir, store) = test_store();
        store.write_file_rel("a.bin", b"xyz").unwrap();
        assert_eq!(store.read_file_rel("a.bin").unwrap(), b"xyz");
        store.delete_file_rel("a.bin").unwrap();
        assert!(matches!(
            store.delete_file_rel("a.bin"),
            Err(NomadError::NotFound(_))
        ));
    }

    #[test]
    fn ensure_default_index_idempotent() {
        let (_dir, store) = test_store();
        store.ensure_default_index("Demo").unwrap();
        let first = store.read_page_rel("index.mu").unwrap();
        store.ensure_default_index("Other").unwrap();
        let second = store.read_page_rel("index.mu").unwrap();
        assert_eq!(first, second);
        assert!(String::from_utf8(first).unwrap().contains("Demo"));
    }

    #[test]
    fn write_rejects_too_large() {
        let dir = tempdir().unwrap();
        let mut roots = NomadContentRoots::under(dir.path());
        roots.max_page_bytes = 4;
        let store = NomadContentStore::new(roots).unwrap();
        let err = store.write_page_rel("x.mu", b"12345").unwrap_err();
        assert!(matches!(err, NomadError::TooLarge { .. }));
    }

    #[test]
    fn read_exact_max_ok_over_max_rejects() {
        let dir = tempdir().unwrap();
        let mut roots = NomadContentRoots::under(dir.path());
        roots.max_page_bytes = 8;
        let store = NomadContentStore::new(roots).unwrap();
        store.write_page_rel("exact.mu", b"12345678").unwrap();
        assert_eq!(store.read_page_rel("exact.mu").unwrap(), b"12345678");
        std::fs::write(dir.path().join("pages/big.mu"), vec![b'x'; 9]).unwrap();
        let err = store.read_page_rel("big.mu").unwrap_err();
        assert!(matches!(err, NomadError::TooLarge { .. }));
    }

    #[test]
    fn read_page_rejects_oversize_without_full_alloc_contract() {
        let dir = tempdir().unwrap();
        let mut roots = NomadContentRoots::under(dir.path());
        roots.max_page_bytes = 8;
        let store = NomadContentStore::new(roots).unwrap();
        store.write_page_rel("tiny.mu", b"ok").unwrap();
        std::fs::write(dir.path().join("pages/big.mu"), vec![b'x'; 32]).unwrap();
        let err = store.read_page_rel("big.mu").unwrap_err();
        assert!(matches!(err, NomadError::TooLarge { .. }));
    }

    #[test]
    fn list_skips_dotfiles_and_allowed_suffix() {
        let (dir, store) = test_store();
        store.write_page_rel("index.mu", b"> ok\n").unwrap();
        std::fs::write(dir.path().join("pages/.hidden.mu"), b"secret").unwrap();
        std::fs::write(dir.path().join("pages/index.mu.allowed"), b"allow").unwrap();
        store.write_file_rel("readme.txt", b"hi").unwrap();
        std::fs::write(dir.path().join("files/.cache"), b"x").unwrap();
        let pages = store.list_pages().unwrap();
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].path, "index.mu");
        let files = store.list_files().unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "readme.txt");
        assert!(store.read_page_rel(".hidden.mu").is_err());
        assert!(store.read_file_rel(".cache").is_err());
    }

    #[test]
    #[cfg(unix)]
    fn list_skips_symlink_dirs_and_respects_depth() {
        use std::os::unix::fs::symlink;
        let (dir, store) = test_store();
        store.write_page_rel("index.mu", b"> ok\n").unwrap();
        let real = dir.path().join("pages/real");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join("nested.mu"), b"n").unwrap();
        symlink(&real, dir.path().join("pages/linkdir")).unwrap();
        let pages = store.list_pages().unwrap();
        assert!(pages.iter().any(|p| p.path == "real/nested.mu"));
        assert!(!pages.iter().any(|p| p.path.starts_with("linkdir")));
    }

    #[test]
    #[cfg(unix)]
    fn ensure_default_index_rejects_dangling_symlink() {
        use std::os::unix::fs::symlink;
        let (dir, store) = test_store();
        let outside = dir.path().join("outside.mu");
        symlink(&outside, dir.path().join("pages/index.mu")).unwrap();
        assert!(matches!(
            store.ensure_default_index("X"),
            Err(NomadError::PathTraversal)
        ));
        assert!(!outside.exists());
    }
}
