use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};

use opentelemetry::{KeyValue, metrics::Counter};
use pingora::upstreams::peer::{Tracer, Tracing};

static NEXT_CONNECTION: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug)]
pub(super) struct Connections {
    active: Arc<Mutex<HashMap<u64, u64>>>,
    events: Counter<u64>,
}

impl Connections {
    pub fn new(events: Counter<u64>) -> Self {
        Self {
            active: Arc::default(),
            events,
        }
    }

    pub fn probe(&self, address: SocketAddr) -> ConnectionProbe {
        ConnectionProbe(Arc::new(ConnectionState {
            id: NEXT_CONNECTION.fetch_add(1, Ordering::Relaxed),
            address,
            connected_at: OnceLock::new(),
            handle: OnceLock::new(),
            connections: self.clone(),
        }))
    }

    pub fn get(&self, handle: u64) -> Option<u64> {
        self.active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&handle)
            .copied()
    }
}

#[derive(Debug)]
struct ConnectionState {
    id: u64,
    address: SocketAddr,
    connected_at: OnceLock<Instant>,
    handle: OnceLock<u64>,
    connections: Connections,
}

// Shared with the stream's cloned tracer, but never owns a request span.
#[derive(Clone, Debug)]
pub(super) struct ConnectionProbe(Arc<ConnectionState>);

impl ConnectionProbe {
    pub fn tracer(&self) -> Tracer {
        Tracer(Box::new(self.clone()))
    }

    pub fn id(&self) -> Option<u64> {
        self.0.connected_at.get().map(|_| self.0.id)
    }

    pub fn bind(&self, handle: u64) -> Option<u64> {
        let id = self.id()?;
        let _ = self.0.handle.set(handle);
        self.0
            .connections
            .active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(handle, id);
        Some(id)
    }
}

impl Tracing for ConnectionProbe {
    fn on_connected(&self) {
        let _ = self.0.connected_at.set(Instant::now());
        self.0
            .connections
            .events
            .add(1, &[KeyValue::new("event", "connected")]);
        tracing::debug!(parent: None, event_kind = "connection", connection_id = self.0.id,
            upstream_address = %self.0.address, "upstream TCP connected");
    }

    fn on_disconnected(&self) {
        if let Some(handle) = self.0.handle.get() {
            let mut active = self
                .0
                .connections
                .active
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            // An old stream must not erase a newer connection using the same handle.
            if active.get(handle) == Some(&self.0.id) {
                active.remove(handle);
            }
        }
        self.0
            .connections
            .events
            .add(1, &[KeyValue::new("event", "released")]);
        tracing::debug!(parent: None, event_kind = "connection", connection_id = self.0.id,
            upstream_address = %self.0.address,
            lifetime_seconds = self.0.connected_at.get().map(|start| start.elapsed().as_secs_f64()),
            "upstream connection released");
    }

    fn boxed_clone(&self) -> Box<dyn Tracing> {
        Box::new(self.clone())
    }
}
