//! Host media conversion cache (`converted_node` analogue).
//!
//! Prefer in-memory LRU. Disk storage is opt-in via an embedder-owned root —
//! never under content `pages/` / `files/`. Embedders must call [`MediaCache::clear`]
//! (or [`NomadNode::clear_media_cache`](crate::NomadNode::clear_media_cache)) to
//! wipe non-memory state.

use std::collections::{HashMap, VecDeque};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::error::NomadError;

/// NomadNet `Node.CONVERSION_CACHE_MAX_FILES`.
pub const DEFAULT_MEDIA_CACHE_MAX_ENTRIES: usize = 256;
/// NomadNet `Node.CONVERSION_CACHE_MAX_BYTES` (192 MiB).
pub const DEFAULT_MEDIA_CACHE_MAX_BYTES: u64 = 192 * 1024 * 1024;

/// Configuration for the host media conversion cache.
#[derive(Debug, Clone)]
pub struct MediaCacheConfig {
    /// When set, converted WebP blobs are also stored under this directory.
    /// Must be provided by the embedder; never derived from content roots.
    pub disk_root: Option<PathBuf>,
    /// Max cached entries (memory + disk index).
    pub max_entries: usize,
    /// Max total cached bytes.
    pub max_bytes: u64,
}

impl Default for MediaCacheConfig {
    fn default() -> Self {
        Self {
            disk_root: None,
            max_entries: DEFAULT_MEDIA_CACHE_MAX_ENTRIES,
            max_bytes: DEFAULT_MEDIA_CACHE_MAX_BYTES,
        }
    }
}

impl MediaCacheConfig {
    /// In-memory only with NomadNet-aligned size caps.
    pub fn memory_only() -> Self {
        Self::default()
    }

    /// Disk-backed cache under an embedder-owned root.
    pub fn with_disk_root(root: impl Into<PathBuf>) -> Self {
        Self {
            disk_root: Some(root.into()),
            ..Self::default()
        }
    }
}

/// Bounded LRU conversion cache (memory default; optional disk mirror).
#[derive(Debug)]
pub struct MediaCache {
    config: MediaCacheConfig,
    /// Newest at back.
    order: VecDeque<String>,
    memory: HashMap<String, Vec<u8>>,
    total_bytes: u64,
}

impl MediaCache {
    /// Create a cache from config. Ensures the disk root exists when configured.
    pub fn new(config: MediaCacheConfig) -> Result<Self, NomadError> {
        if let Some(root) = &config.disk_root {
            fs::create_dir_all(root)?;
        }
        Ok(Self {
            config,
            order: VecDeque::new(),
            memory: HashMap::new(),
            total_bytes: 0,
        })
    }

    /// Borrow the active config.
    pub fn config(&self) -> &MediaCacheConfig {
        &self.config
    }

    /// Look up a cache key (memory first, then disk).
    pub fn get(&mut self, key: &str) -> Result<Option<Vec<u8>>, NomadError> {
        if self.memory.contains_key(key) {
            self.touch(key);
            return Ok(self.memory.get(key).cloned());
        }
        if let Some(root) = &self.config.disk_root {
            let path = root.join(key);
            if path.is_file() {
                let bytes = read_file(&path)?;
                // Promote into memory for subsequent hits.
                self.insert_memory(key.to_string(), bytes.clone());
                self.touch_mtime(&path);
                return Ok(Some(bytes));
            }
        }
        Ok(None)
    }

    /// Store converted bytes under `key` (e.g. `{sha}.q{q}.d{d}.webp`).
    pub fn insert(&mut self, key: String, bytes: Vec<u8>) -> Result<(), NomadError> {
        if let Some(root) = &self.config.disk_root {
            let path = root.join(&key);
            atomic_write(&path, &bytes)?;
        }
        self.insert_memory(key, bytes);
        self.prune()?;
        Ok(())
    }

    /// Remove one key from memory and disk (if configured).
    pub fn clear_key(&mut self, key: &str) -> Result<(), NomadError> {
        if let Some(old) = self.memory.remove(key) {
            self.total_bytes = self.total_bytes.saturating_sub(old.len() as u64);
        }
        self.order.retain(|k| k != key);
        if let Some(root) = &self.config.disk_root {
            let path = root.join(key);
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(NomadError::Io(e)),
            }
        }
        Ok(())
    }

    /// Wipe all memory entries and, when configured, all files under the disk root.
    pub fn clear(&mut self) -> Result<(), NomadError> {
        self.memory.clear();
        self.order.clear();
        self.total_bytes = 0;
        if let Some(root) = &self.config.disk_root {
            if root.is_dir() {
                for entry in fs::read_dir(root)? {
                    let entry = entry?;
                    let path = entry.path();
                    if path.is_file() {
                        fs::remove_file(&path)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Number of in-memory entries (tests / stats).
    pub fn memory_len(&self) -> usize {
        self.memory.len()
    }

    /// Approximate in-memory byte total.
    pub fn memory_bytes(&self) -> u64 {
        self.total_bytes
    }

    fn insert_memory(&mut self, key: String, bytes: Vec<u8>) {
        if let Some(old) = self.memory.remove(&key) {
            self.total_bytes = self.total_bytes.saturating_sub(old.len() as u64);
            self.order.retain(|k| k != &key);
        }
        self.total_bytes = self.total_bytes.saturating_add(bytes.len() as u64);
        self.memory.insert(key.clone(), bytes);
        self.order.push_back(key);
        while self.memory.len() > self.config.max_entries
            || self.total_bytes > self.config.max_bytes
        {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(old) = self.memory.remove(&oldest) {
                self.total_bytes = self.total_bytes.saturating_sub(old.len() as u64);
            }
        }
    }

    fn touch(&mut self, key: &str) {
        if self.memory.contains_key(key) {
            self.order.retain(|k| k != key);
            self.order.push_back(key.to_string());
        }
    }

    fn touch_mtime(&self, path: &Path) {
        let now = SystemTime::now();
        let _ = filetime_set(path, now);
    }

    fn prune(&mut self) -> Result<(), NomadError> {
        // Memory already pruned in insert_memory.
        let Some(root) = &self.config.disk_root else {
            return Ok(());
        };
        if !root.is_dir() {
            return Ok(());
        }
        let mut entries: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
        let mut total = 0u64;
        for entry in fs::read_dir(root)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let meta = entry.metadata()?;
            let len = meta.len();
            if len == 0 {
                continue;
            }
            let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            total = total.saturating_add(len);
            entries.push((mtime, len, path));
        }
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        while entries.len() > self.config.max_entries || total > self.config.max_bytes {
            let Some((_, size, path)) = entries.first().cloned() else {
                break;
            };
            entries.remove(0);
            total = total.saturating_sub(size);
            let _ = fs::remove_file(&path);
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if let Some(old) = self.memory.remove(name) {
                    self.total_bytes = self.total_bytes.saturating_sub(old.len() as u64);
                }
                self.order.retain(|k| k != name);
            }
        }
        Ok(())
    }
}

fn read_file(path: &Path) -> Result<Vec<u8>, NomadError> {
    let mut f = File::open(path)?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    Ok(buf)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), NomadError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    {
        let mut f = File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

fn filetime_set(path: &Path, at: SystemTime) -> std::io::Result<()> {
    // Best-effort LRU hint for disk prune; failures are ignored by callers.
    #[cfg(unix)]
    {
        let secs = at
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs() as libc::time_t)
            .unwrap_or(0);
        let times = [
            libc::timeval {
                tv_sec: secs,
                tv_usec: 0,
            },
            libc::timeval {
                tv_sec: secs,
                tv_usec: 0,
            },
        ];
        use std::os::unix::ffi::OsStrExt;
        let c_path = std::ffi::CString::new(path.as_os_str().as_bytes())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
        let rc = unsafe { libc::utimes(c_path.as_ptr(), times.as_ptr()) };
        if rc != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (path, at);
        Ok(())
    }
}

/// Build NomadNet-compatible conversion cache file name.
pub fn conversion_cache_key(source_sha256_hex: &str, quality: u8, max_dimension: u32) -> String {
    format!("{source_sha256_hex}.q{quality}.d{max_dimension}.webp")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn memory_lru_evicts_oldest() {
        let mut cache = MediaCache::new(MediaCacheConfig {
            disk_root: None,
            max_entries: 2,
            max_bytes: DEFAULT_MEDIA_CACHE_MAX_BYTES,
        })
        .unwrap();
        cache.insert("a.webp".into(), b"1".to_vec()).unwrap();
        cache.insert("b.webp".into(), b"2".to_vec()).unwrap();
        cache.insert("c.webp".into(), b"3".to_vec()).unwrap();
        assert_eq!(cache.memory_len(), 2);
        assert!(cache.get("a.webp").unwrap().is_none());
        assert_eq!(cache.get("b.webp").unwrap().as_deref(), Some(b"2".as_slice()));
        assert_eq!(cache.get("c.webp").unwrap().as_deref(), Some(b"3".as_slice()));
    }

    #[test]
    fn clear_wipes_memory_and_disk() {
        let dir = TempDir::new().unwrap();
        let mut cache = MediaCache::new(MediaCacheConfig::with_disk_root(dir.path())).unwrap();
        cache
            .insert("x.q85.d1200.webp".into(), b"webp".to_vec())
            .unwrap();
        assert!(dir.path().join("x.q85.d1200.webp").is_file());
        cache.clear().unwrap();
        assert_eq!(cache.memory_len(), 0);
        assert!(!dir.path().join("x.q85.d1200.webp").exists());
    }

    #[test]
    fn clear_key_removes_one() {
        let mut cache = MediaCache::new(MediaCacheConfig::memory_only()).unwrap();
        cache.insert("a".into(), b"1".to_vec()).unwrap();
        cache.insert("b".into(), b"2".to_vec()).unwrap();
        cache.clear_key("a").unwrap();
        assert!(cache.get("a").unwrap().is_none());
        assert_eq!(cache.get("b").unwrap().as_deref(), Some(b"2".as_slice()));
    }

    #[test]
    fn disk_get_promotes_to_memory() {
        let dir = TempDir::new().unwrap();
        let key = "k.q85.d1200.webp";
        fs::write(dir.path().join(key), b"from-disk").unwrap();
        let mut cache = MediaCache::new(MediaCacheConfig::with_disk_root(dir.path())).unwrap();
        assert_eq!(
            cache.get(key).unwrap().as_deref(),
            Some(b"from-disk".as_slice())
        );
        assert_eq!(cache.memory_len(), 1);
    }
}
