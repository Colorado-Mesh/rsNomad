//! Live Nomad node: register destination, serve pages/files, announce.
//!
//! The LinkManager request handler is invoked synchronously on the link event
//! loop. Serve paths therefore perform bounded synchronous filesystem reads
//! (with size caps). Callers that need non-blocking I/O should keep content
//! small or host behind a process that isolates disk stalls.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use rns_identity::identity::Identity;
use rns_runtime::link_manager::{LinkManager, RequestOutcome, register_destination};
use rns_transport::messages::TransportMessage;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::announce::{nomad_destination_hash, send_nomad_announce, send_nomad_announce_try};
use crate::error::NomadError;
use crate::micron::not_found_page;
use crate::paths::{
    DEFAULT_INDEX_ROUTE, FILE_PREFIX, NOMAD_NODE_ASPECT, PAGE_PREFIX, normalize_file_route,
    normalize_page_route, path_hash,
};
use crate::storage::NomadContentStore;

/// Max concurrent request handlers (disk/network budget).
const MAX_IN_FLIGHT_REQUESTS: u64 = 8;
/// Max requests accepted per fixed window.
const MAX_REQUESTS_PER_WINDOW: u64 = 60;
const REQUEST_WINDOW: Duration = Duration::from_secs(10);
/// Max UTF-8 bytes retained for announce / display name.
const MAX_DISPLAY_NAME_BYTES: usize = 256;
/// Timeout for awaited announce sends on the periodic ticker.
const ANNOUNCE_SEND_TIMEOUT: Duration = Duration::from_secs(5);

/// Configuration for [`NomadNode::spawn`].
#[derive(Debug, Clone)]
pub struct NomadNodeConfig {
    /// UTF-8 display name announced as app data (truncated to 256 bytes).
    pub display_name: String,
    /// Periodic announce interval; `None` disables the ticker.
    pub announce_interval: Option<Duration>,
    /// Send an announce immediately after spawn.
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

/// Snapshot of serve counters.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NomadServeStats {
    /// Total requests admitted (including not-found).
    pub request_count: u64,
    /// Successful page replies.
    pub page_hits: u64,
    /// Successful file replies.
    pub file_hits: u64,
    /// Missing routes / missing content.
    pub not_found_count: u64,
    /// Wall-clock ms of the last admitted request, if any.
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

fn rebuild_routes(routes: &mut RouteTable, store: &NomadContentStore) -> Result<(), NomadError> {
    routes.clear();
    for page in store.list_pages()? {
        routes.register(normalize_page_route(&page.path)?)?;
    }
    for file in store.list_files()? {
        routes.register(normalize_file_route(&file.path)?)?;
    }
    // Always register index even if list was empty before ensure.
    routes.register(DEFAULT_INDEX_ROUTE.into())?;
    Ok(())
}

struct SharedState {
    display_name: Mutex<String>,
    store: NomadContentStore,
    routes: RwLock<RouteTable>,
    stats: NomadServeStatsInner,
    budget: RequestBudget,
}

struct RequestBudgetState {
    in_flight: u64,
    window_start: Instant,
    window_count: u64,
}

struct RequestBudget {
    state: Mutex<RequestBudgetState>,
}

impl RequestBudget {
    fn new() -> Self {
        Self {
            state: Mutex::new(RequestBudgetState {
                in_flight: 0,
                window_start: Instant::now(),
                window_count: 0,
            }),
        }
    }

    /// Try to admit one request. Returns a guard that decrements in-flight on drop.
    fn try_acquire(&self) -> Option<RequestBudgetGuard<'_>> {
        let mut state = self.state.lock().ok()?;
        let now = Instant::now();
        if now.duration_since(state.window_start) >= REQUEST_WINDOW {
            state.window_start = now;
            state.window_count = 0;
        }
        if state.window_count >= MAX_REQUESTS_PER_WINDOW {
            return None;
        }
        if state.in_flight >= MAX_IN_FLIGHT_REQUESTS {
            return None;
        }
        state.window_count += 1;
        state.in_flight += 1;
        Some(RequestBudgetGuard { budget: self })
    }
}

struct RequestBudgetGuard<'a> {
    budget: &'a RequestBudget,
}

impl Drop for RequestBudgetGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut state) = self.budget.state.lock() {
            state.in_flight = state.in_flight.saturating_sub(1);
        }
    }
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

fn clamp_display_name(name: impl Into<String>) -> String {
    let mut name = name.into();
    name.retain(|c| !c.is_control());
    if name.len() > MAX_DISPLAY_NAME_BYTES {
        // Truncate on a UTF-8 char boundary.
        let mut end = MAX_DISPLAY_NAME_BYTES;
        while end > 0 && !name.is_char_boundary(end) {
            end -= 1;
        }
        name.truncate(end);
    }
    name
}

fn shared_display_name(shared: &SharedState) -> String {
    shared
        .display_name
        .lock()
        .map(|g| g.clone())
        .unwrap_or_else(|e| {
            tracing::warn!("display_name lock poisoned; using empty name");
            e.into_inner().clone()
        })
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
        let display_name = clamp_display_name(config.display_name);
        store.ensure_default_index(&display_name)?;

        let destination_hash = nomad_destination_hash(&identity);
        let event_rx = register_destination(&transport_tx, destination_hash, NOMAD_NODE_ASPECT);

        let shared = Arc::new(SharedState {
            display_name: Mutex::new(display_name),
            store,
            routes: RwLock::new(RouteTable::new()),
            stats: NomadServeStatsInner::new(),
            budget: RequestBudget::new(),
        });

        // Pre-register known filesystem pages/files for path-hash lookup.
        {
            let mut routes = shared
                .routes
                .write()
                .map_err(|_| NomadError::message("routes lock poisoned"))?;
            rebuild_routes(&mut routes, &shared.store)?;
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
        // Request body (`_data`) is ignored: static hosting only. Callers that
        // need form fields should decode with `decode_request_fields` themselves.
        link_mgr.set_request_handler_ex(move |_link_id, path_hash, _data| {
            handle_request(&handler_shared, path_hash)
        });

        let announce_tx = transport_tx.clone();
        let announce_identity = identity.clone();
        let announce_name = shared.clone();
        link_mgr.set_announce_handler(move || {
            let name = shared_display_name(&announce_name);
            send_nomad_announce_try(&announce_tx, &announce_identity, Some(name.as_str()));
        });

        let link_task = tokio::spawn(async move {
            link_mgr.run().await;
            tracing::warn!("nomad link manager task exited");
        });

        let announce_task = config.announce_interval.map(|interval| {
            let tx = transport_tx.clone();
            let id = identity.clone();
            let state = shared.clone();
            tokio::spawn(async move {
                let mut ticker = tokio::time::interval(interval);
                ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                loop {
                    ticker.tick().await;
                    let name = shared_display_name(&state);
                    match tokio::time::timeout(
                        ANNOUNCE_SEND_TIMEOUT,
                        send_nomad_announce(&tx, &id, Some(name.as_str())),
                    )
                    .await
                    {
                        Ok(Ok(())) => {}
                        Ok(Err(e)) => {
                            tracing::warn!(error = %e, "nomad periodic announce failed");
                            // Permanent channel closure: stop the ticker.
                            if e.to_string().contains("transport channel closed") {
                                break;
                            }
                        }
                        Err(_) => {
                            tracing::warn!("nomad periodic announce timed out");
                        }
                    }
                }
                tracing::warn!("nomad announce task exited");
            })
        });

        // Construct Self before the optional startup announce so cancellation/Drop
        // can abort the link task (no untracked zombie destination).
        let node = Self {
            destination_hash,
            identity_hash: identity.hash,
            shared,
            transport_tx,
            identity,
            _link_task: link_task,
            _announce_task: announce_task,
        };
        if config.announce_at_start {
            if let Err(e) = node.announce_now().await {
                tracing::warn!(error = %e, "nomad startup announce failed");
            }
        }
        Ok(node)
    }

    /// Destination hash for `nomadnetwork.node`.
    pub fn destination_hash(&self) -> [u8; 16] {
        self.destination_hash
    }

    /// Identity hash of the hosting identity.
    pub fn identity_hash(&self) -> [u8; 16] {
        self.identity_hash
    }

    /// Hex-encoded destination hash.
    pub fn destination_hash_hex(&self) -> String {
        hex::encode(self.destination_hash)
    }

    /// Hex-encoded identity hash.
    pub fn identity_hash_hex(&self) -> String {
        hex::encode(self.identity_hash)
    }

    /// Current display name used in announces.
    pub fn display_name(&self) -> String {
        shared_display_name(&self.shared)
    }

    /// Update the display name (clamped / control-stripped). Call `announce_now`
    /// to publish the change immediately.
    pub fn set_display_name(&self, name: impl Into<String>) {
        if let Ok(mut guard) = self.shared.display_name.lock() {
            *guard = clamp_display_name(name);
        } else {
            tracing::warn!("display_name lock poisoned; set_display_name ignored");
        }
    }

    /// Snapshot of serve counters.
    pub fn stats(&self) -> NomadServeStats {
        self.shared.stats.snapshot()
    }

    /// Borrow the content store (write pages/files, then [`Self::reload_routes`]).
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
        rebuild_routes(&mut routes, &self.shared.store)
    }

    /// Send one announce with the current display name.
    pub async fn announce_now(&self) -> Result<(), NomadError> {
        let name = self.display_name();
        tokio::time::timeout(
            ANNOUNCE_SEND_TIMEOUT,
            send_nomad_announce(&self.transport_tx, &self.identity, Some(name.as_str())),
        )
        .await
        .map_err(|_| NomadError::message("announce send timed out"))?
    }

    /// Abort background tasks and drop the node.
    pub fn shutdown(self) {
        drop(self);
    }
}

impl Drop for NomadNode {
    fn drop(&mut self) {
        self._link_task.abort();
        if let Some(task) = self._announce_task.take() {
            task.abort();
        }
    }
}

/// Look up a registered route by wire path hash.
///
/// Misses are definitive 404s — callers must use [`NomadNode::reload_routes`]
/// after content CRUD. Unknown hashes must not trigger filesystem walks
/// (remote DoS amplification).
fn lookup_route(shared: &SharedState, path_hash_bytes: [u8; 16]) -> Option<String> {
    shared
        .routes
        .read()
        .ok()?
        .by_hash
        .get(&path_hash_bytes)
        .cloned()
}

fn handle_request(shared: &SharedState, path_hash_bytes: [u8; 16]) -> RequestOutcome {
    let Some(_budget) = shared.budget.try_acquire() else {
        tracing::warn!("nomad request budget exceeded; dropping request");
        return RequestOutcome::Drop;
    };

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    shared.stats.request_count.fetch_add(1, Ordering::Relaxed);
    shared
        .stats
        .last_request_ms
        .store(now_ms, Ordering::Relaxed);

    let route = lookup_route(shared, path_hash_bytes);

    let Some(route) = route else {
        shared.stats.not_found_count.fetch_add(1, Ordering::Relaxed);
        return RequestOutcome::Reply(not_found_page("/page/unknown").into_bytes());
    };

    if route.starts_with(PAGE_PREFIX) {
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
    } else if route.starts_with(FILE_PREFIX) {
        match shared.store.read_file_route(&route) {
            Ok(bytes) => {
                shared.stats.file_hits.fetch_add(1, Ordering::Relaxed);
                RequestOutcome::Reply(bytes)
            }
            Err(NomadError::NotFound(_)) => {
                // Files have no Micron 404 body — drop silently (NomadNet parity).
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::NomadContentRoots;
    use rns_runtime::link_manager::RequestOutcome;
    use tempfile::TempDir;

    fn shared_with_content(
        dir: &TempDir,
        pages: &[(&str, &[u8])],
        files: &[(&str, &[u8])],
    ) -> Arc<SharedState> {
        let store = NomadContentStore::new(NomadContentRoots::under(dir.path())).unwrap();
        for (path, body) in pages {
            store.write_page_rel(path, body).unwrap();
        }
        for (path, body) in files {
            store.write_file_rel(path, body).unwrap();
        }
        let shared = Arc::new(SharedState {
            display_name: Mutex::new("Test".into()),
            store,
            routes: RwLock::new(RouteTable::new()),
            stats: NomadServeStatsInner::new(),
            budget: RequestBudget::new(),
        });
        {
            let mut routes = shared.routes.write().unwrap();
            rebuild_routes(&mut routes, &shared.store).unwrap();
        }
        shared
    }

    #[test]
    fn link_request_handler_serves_page_and_file() {
        let dir = TempDir::new().unwrap();
        let shared = shared_with_content(
            &dir,
            &[("index.mu", b"> Hello from host\n")],
            &[("readme.txt", b"file-bytes")],
        );

        let page_hash = path_hash("/page/index.mu");
        match handle_request(&shared, page_hash) {
            RequestOutcome::Reply(bytes) => {
                assert_eq!(bytes, b"> Hello from host\n");
            }
            _ => panic!("expected page reply from link request handler"),
        }

        let file_hash = path_hash("/file/readme.txt");
        match handle_request(&shared, file_hash) {
            RequestOutcome::Reply(bytes) => {
                assert_eq!(bytes, b"file-bytes");
            }
            _ => panic!("expected file reply from link request handler"),
        }

        let stats = shared.stats.snapshot();
        assert_eq!(stats.page_hits, 1);
        assert_eq!(stats.file_hits, 1);
        assert_eq!(stats.request_count, 2);
    }

    #[test]
    fn unknown_path_hash_does_not_clear_or_rescan_routes() {
        let dir = TempDir::new().unwrap();
        let shared = shared_with_content(&dir, &[("index.mu", b"> ok\n")], &[]);
        let before = shared.routes.read().unwrap().by_hash.len();
        assert!(before >= 1);

        match handle_request(&shared, [0u8; 16]) {
            RequestOutcome::Reply(bytes) => {
                let body = String::from_utf8_lossy(&bytes);
                assert!(body.contains("Not found"));
            }
            _ => panic!("expected not-found Micron reply"),
        }

        let after = shared.routes.read().unwrap().by_hash.len();
        assert_eq!(before, after, "soft-miss must not wipe the route table");

        // Registered page still serves without requiring a rebuild.
        match handle_request(&shared, path_hash("/page/index.mu")) {
            RequestOutcome::Reply(bytes) => assert_eq!(bytes, b"> ok\n"),
            _ => panic!("expected page reply after unknown-hash miss"),
        }
    }

    #[test]
    fn missing_file_drops_without_reply() {
        let dir = TempDir::new().unwrap();
        let shared = shared_with_content(&dir, &[("index.mu", b"> ok\n")], &[]);
        {
            let mut routes = shared.routes.write().unwrap();
            routes.register("/file/gone.bin".into()).unwrap();
        }
        match handle_request(&shared, path_hash("/file/gone.bin")) {
            RequestOutcome::Drop => {}
            _ => panic!("expected Drop for missing file"),
        }
        assert_eq!(shared.stats.not_found_count.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn request_budget_bounds_in_flight_concurrency() {
        let budget = RequestBudget::new();
        let mut guards = Vec::new();
        for _ in 0..MAX_IN_FLIGHT_REQUESTS {
            guards.push(budget.try_acquire().expect("admit under cap"));
        }
        assert!(
            budget.try_acquire().is_none(),
            "must reject over concurrency"
        );
        drop(guards);
        assert!(
            budget.try_acquire().is_some(),
            "must admit again after in-flight drain"
        );
    }

    #[test]
    fn request_budget_bounds_window_count() {
        let budget = RequestBudget::new();
        for _ in 0..MAX_REQUESTS_PER_WINDOW {
            // Drop immediately so in-flight stays under the concurrency cap;
            // window_count still accumulates for the fixed window.
            assert!(budget.try_acquire().is_some());
        }
        assert!(budget.try_acquire().is_none(), "must reject over window");
    }

    #[test]
    fn link_request_handler_skips_unregistered_dotfile_routes() {
        let dir = TempDir::new().unwrap();
        let shared = shared_with_content(&dir, &[("index.mu", b"> ok\n")], &[]);
        // Forbidden routes are not registered; handler returns the not-found Micron page.
        let forbidden = path_hash("/page/.secret.mu");
        match handle_request(&shared, forbidden) {
            RequestOutcome::Reply(bytes) => {
                let body = String::from_utf8_lossy(&bytes);
                assert!(body.contains("Not found"));
            }
            RequestOutcome::Drop => panic!("expected not-found reply"),
            _ => panic!("unexpected request outcome"),
        }
        assert_eq!(shared.stats.page_hits.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn reload_routes_picks_up_new_page() {
        let dir = TempDir::new().unwrap();
        let shared = shared_with_content(&dir, &[("index.mu", b"> ok\n")], &[]);
        shared
            .store
            .write_page_rel("extra.mu", b"> extra\n")
            .unwrap();
        // Not registered yet.
        match handle_request(&shared, path_hash("/page/extra.mu")) {
            RequestOutcome::Reply(bytes) => {
                assert!(String::from_utf8_lossy(&bytes).contains("Not found"));
            }
            _ => panic!("expected not-found before reload"),
        }
        {
            let mut routes = shared.routes.write().unwrap();
            rebuild_routes(&mut routes, &shared.store).unwrap();
        }
        match handle_request(&shared, path_hash("/page/extra.mu")) {
            RequestOutcome::Reply(bytes) => assert_eq!(bytes, b"> extra\n"),
            _ => panic!("expected reply after reload"),
        }
    }
}
