//! Live Nomad node: register destination, serve pages/files, announce.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use rns_identity::identity::Identity;
use rns_runtime::link_manager::{LinkManager, RequestOutcome, register_destination};
use rns_transport::messages::TransportMessage;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::announce::{nomad_destination_hash, send_nomad_announce, send_nomad_announce_try};
use crate::error::NomadError;
use crate::micron::not_found_page;
use crate::paths::{NOMAD_NODE_ASPECT, normalize_file_route, normalize_page_route, path_hash};
use crate::storage::NomadContentStore;

#[derive(Debug, Clone)]
pub struct NomadNodeConfig {
    pub display_name: String,
    pub announce_interval: Option<Duration>,
    pub announce_at_start: bool,
}

impl Default for NomadNodeConfig {
    fn default() -> Self {
        Self {
            display_name: "Nomad node".into(),
            announce_interval: Some(Duration::from_secs(3600)),
            announce_at_start: true,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NomadServeStats {
    pub request_count: u64,
    pub page_hits: u64,
    pub file_hits: u64,
    pub not_found_count: u64,
    pub last_request_ms: Option<u64>,
}

struct RouteTable {
    /// path_hash → absolute route string (`/page/...` or `/file/...`)
    by_hash: HashMap<[u8; 16], String>,
}

impl RouteTable {
    fn new() -> Self {
        Self {
            by_hash: HashMap::new(),
        }
    }

    fn register(&mut self, route: String) -> Result<(), NomadError> {
        let hash = path_hash(&route);
        if let Some(existing) = self.by_hash.get(&hash) {
            if existing != &route {
                return Err(NomadError::message(format!(
                    "route hash collision between {existing} and {route}"
                )));
            }
        }
        self.by_hash.insert(hash, route);
        Ok(())
    }

    fn clear(&mut self) {
        self.by_hash.clear();
    }
}

struct SharedState {
    display_name: Mutex<String>,
    store: NomadContentStore,
    routes: RwLock<RouteTable>,
    stats: NomadServeStatsInner,
}

struct NomadServeStatsInner {
    request_count: AtomicU64,
    page_hits: AtomicU64,
    file_hits: AtomicU64,
    not_found_count: AtomicU64,
    last_request_ms: AtomicU64,
}

impl NomadServeStatsInner {
    fn new() -> Self {
        Self {
            request_count: AtomicU64::new(0),
            page_hits: AtomicU64::new(0),
            file_hits: AtomicU64::new(0),
            not_found_count: AtomicU64::new(0),
            last_request_ms: AtomicU64::new(0),
        }
    }

    fn snapshot(&self) -> NomadServeStats {
        let last = self.last_request_ms.load(Ordering::Relaxed);
        NomadServeStats {
            request_count: self.request_count.load(Ordering::Relaxed),
            page_hits: self.page_hits.load(Ordering::Relaxed),
            file_hits: self.file_hits.load(Ordering::Relaxed),
            not_found_count: self.not_found_count.load(Ordering::Relaxed),
            last_request_ms: if last == 0 { None } else { Some(last) },
        }
    }
}

/// Running Nomad Network page/file host.
pub struct NomadNode {
    destination_hash: [u8; 16],
    identity_hash: [u8; 16],
    shared: Arc<SharedState>,
    transport_tx: mpsc::Sender<TransportMessage>,
    identity: Identity,
    _link_task: JoinHandle<()>,
    _announce_task: Option<JoinHandle<()>>,
}

impl NomadNode {
    /// Register `nomadnetwork.node`, start LinkManager request handler, optional announce loop.
    pub async fn spawn(
        transport_tx: mpsc::Sender<TransportMessage>,
        identity: Identity,
        store: NomadContentStore,
        config: NomadNodeConfig,
    ) -> Result<Self, NomadError> {
        store.ensure_default_index(&config.display_name)?;

        let destination_hash = nomad_destination_hash(&identity);
        let event_rx = register_destination(&transport_tx, destination_hash, NOMAD_NODE_ASPECT);

        let shared = Arc::new(SharedState {
            display_name: Mutex::new(config.display_name.clone()),
            store,
            routes: RwLock::new(RouteTable::new()),
            stats: NomadServeStatsInner::new(),
        });

        // Pre-register known filesystem pages/files for path-hash lookup.
        {
            let mut routes = shared
                .routes
                .write()
                .map_err(|_| NomadError::message("routes lock poisoned"))?;
            for page in shared.store.list_pages()? {
                let route = normalize_page_route(&page.path)?;
                routes.register(route)?;
            }
            for file in shared.store.list_files()? {
                let route = normalize_file_route(&file.path)?;
                routes.register(route)?;
            }
            // Always register index even if list was empty before ensure.
            routes.register("/page/index.mu".into())?;
        }

        let signing_key = identity
            .get_signing_key()
            .ok_or_else(|| NomadError::message("identity has no signing key"))?;

        let mut link_mgr = LinkManager::with_destination(
            transport_tx.clone(),
            event_rx,
            &identity,
            NOMAD_NODE_ASPECT,
            Some(signing_key),
        );

        let handler_shared = shared.clone();
        link_mgr.set_request_handler_ex(move |_link_id, path_hash, _data| {
            handle_request(&handler_shared, path_hash)
        });

        let announce_tx = transport_tx.clone();
        let announce_identity = identity.clone();
        let announce_name = shared.clone();
        link_mgr.set_announce_handler(move || {
            let name = announce_name
                .display_name
                .lock()
                .ok()
                .map(|g| g.clone())
                .unwrap_or_default();
            send_nomad_announce_try(&announce_tx, &announce_identity, Some(name.as_str()));
        });

        let link_task = tokio::spawn(async move {
            link_mgr.run().await;
        });

        if config.announce_at_start {
            let _ =
                send_nomad_announce(&transport_tx, &identity, Some(config.display_name.as_str()))
                    .await;
        }

        let announce_task = config.announce_interval.map(|interval| {
            let tx = transport_tx.clone();
            let id = identity.clone();
            let state = shared.clone();
            tokio::spawn(async move {
                let mut ticker = tokio::time::interval(interval);
                ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                loop {
                    ticker.tick().await;
                    let name = state
                        .display_name
                        .lock()
                        .ok()
                        .map(|g| g.clone())
                        .unwrap_or_default();
                    let _ = send_nomad_announce(&tx, &id, Some(name.as_str())).await;
                }
            })
        });

        Ok(Self {
            destination_hash,
            identity_hash: identity.hash,
            shared,
            transport_tx,
            identity,
            _link_task: link_task,
            _announce_task: announce_task,
        })
    }

    pub fn destination_hash(&self) -> [u8; 16] {
        self.destination_hash
    }

    pub fn identity_hash(&self) -> [u8; 16] {
        self.identity_hash
    }

    pub fn destination_hash_hex(&self) -> String {
        hex::encode(self.destination_hash)
    }

    pub fn identity_hash_hex(&self) -> String {
        hex::encode(self.identity_hash)
    }

    pub fn display_name(&self) -> String {
        self.shared
            .display_name
            .lock()
            .map(|g| g.clone())
            .unwrap_or_default()
    }

    pub fn set_display_name(&self, name: impl Into<String>) {
        if let Ok(mut guard) = self.shared.display_name.lock() {
            *guard = name.into();
        }
    }

    pub fn stats(&self) -> NomadServeStats {
        self.shared.stats.snapshot()
    }

    pub fn store(&self) -> &NomadContentStore {
        &self.shared.store
    }

    /// Refresh route table from the filesystem (call after CRUD).
    pub fn reload_routes(&self) -> Result<(), NomadError> {
        let mut routes = self
            .shared
            .routes
            .write()
            .map_err(|_| NomadError::message("routes lock poisoned"))?;
        routes.clear();
        for page in self.shared.store.list_pages()? {
            routes.register(normalize_page_route(&page.path)?)?;
        }
        for file in self.shared.store.list_files()? {
            routes.register(normalize_file_route(&file.path)?)?;
        }
        routes.register("/page/index.mu".into())?;
        Ok(())
    }

    pub async fn announce_now(&self) -> Result<(), NomadError> {
        let name = self.display_name();
        send_nomad_announce(&self.transport_tx, &self.identity, Some(name.as_str())).await
    }

    pub fn shutdown(self) {
        self._link_task.abort();
        if let Some(task) = self._announce_task {
            task.abort();
        }
    }
}

fn lookup_route(shared: &SharedState, path_hash_bytes: [u8; 16]) -> Option<String> {
    if let Ok(routes) = shared.routes.read() {
        if let Some(route) = routes.by_hash.get(&path_hash_bytes).cloned() {
            return Some(route);
        }
    }
    // Soft miss: rescan filesystem (covers CRUD that forgot reload_routes).
    if let Ok(mut routes) = shared.routes.write() {
        routes.clear();
        if let Ok(pages) = shared.store.list_pages() {
            for page in pages {
                if let Ok(route) = normalize_page_route(&page.path) {
                    let _ = routes.register(route);
                }
            }
        }
        if let Ok(files) = shared.store.list_files() {
            for file in files {
                if let Ok(route) = normalize_file_route(&file.path) {
                    let _ = routes.register(route);
                }
            }
        }
        let _ = routes.register("/page/index.mu".into());
        return routes.by_hash.get(&path_hash_bytes).cloned();
    }
    None
}

fn handle_request(shared: &SharedState, path_hash_bytes: [u8; 16]) -> RequestOutcome {
    shared.stats.request_count.fetch_add(1, Ordering::Relaxed);
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    shared
        .stats
        .last_request_ms
        .store(now_ms, Ordering::Relaxed);

    let route = lookup_route(shared, path_hash_bytes);

    let Some(route) = route else {
        shared.stats.not_found_count.fetch_add(1, Ordering::Relaxed);
        return RequestOutcome::Reply(not_found_page("/page/unknown").into_bytes());
    };

    if route.starts_with("/page/") {
        match shared.store.read_page_route(&route) {
            Ok(bytes) => {
                shared.stats.page_hits.fetch_add(1, Ordering::Relaxed);
                RequestOutcome::Reply(bytes)
            }
            Err(NomadError::NotFound(_)) => {
                shared.stats.not_found_count.fetch_add(1, Ordering::Relaxed);
                RequestOutcome::Reply(not_found_page(&route).into_bytes())
            }
            Err(e) => {
                tracing::warn!(error = %e, route = %route, "nomad page serve failed");
                RequestOutcome::Drop
            }
        }
    } else if route.starts_with("/file/") {
        match shared.store.read_file_route(&route) {
            Ok(bytes) => {
                shared.stats.file_hits.fetch_add(1, Ordering::Relaxed);
                RequestOutcome::Reply(bytes)
            }
            Err(NomadError::NotFound(_)) => {
                shared.stats.not_found_count.fetch_add(1, Ordering::Relaxed);
                RequestOutcome::Drop
            }
            Err(e) => {
                tracing::warn!(error = %e, route = %route, "nomad file serve failed");
                RequestOutcome::Drop
            }
        }
    } else {
        RequestOutcome::Drop
    }
}
