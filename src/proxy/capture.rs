//! In-memory traffic capture store (ring buffer).

use std::collections::VecDeque;
use std::sync::{Arc, RwLock};

/// One captured HTTP transaction.
#[derive(Debug, Clone)]
pub struct CaptureEntry {
    /// Monotonic id (increasing per request).
    pub id: u64,
    /// ISO-ish timestamp (ms since proxy start).
    pub ts_ms: u64,
    pub method: String,
    pub url: String,
    pub status: u16,
    /// Round-trip time in milliseconds.
    pub duration_ms: u64,
    pub req_headers: Vec<(String, String)>,
    pub req_body: Vec<u8>,
    pub resp_headers: Vec<(String, String)>,
    pub resp_body: Vec<u8>,
}

/// Thread-safe ring buffer of captured entries. Uses a std::sync::RwLock so
/// it's safe to access from either smol or tokio contexts.
#[derive(Clone)]
pub struct CaptureStore {
    inner: Arc<RwLock<CaptureInner>>,
}

struct CaptureInner {
    next_id: u64,
    start_ms: u64,
    entries: VecDeque<CaptureEntry>,
    cap: usize,
}

impl CaptureStore {
    pub fn new(cap: usize) -> Self {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        Self {
            inner: Arc::new(RwLock::new(CaptureInner {
                next_id: 0,
                start_ms: now_ms,
                entries: VecDeque::with_capacity(cap),
                cap,
            })),
        }
    }

    pub fn push(&self, mut e: CaptureEntry) {
        if let Ok(mut g) = self.inner.write() {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            e.id = g.next_id;
            g.next_id += 1;
            e.ts_ms = now_ms.saturating_sub(g.start_ms);
            g.entries.push_back(e);
            while g.entries.len() > g.cap {
                g.entries.pop_front();
            }
        }
    }

    /// Snapshot all entries (ordered oldest → newest).
    pub fn snapshot(&self) -> Vec<CaptureEntry> {
        self.inner
            .read()
            .map(|g| g.entries.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub fn clear(&self) {
        if let Ok(mut g) = self.inner.write() {
            g.entries.clear();
            g.next_id = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn push_evicts_oldest() {
        let store = CaptureStore::new(2);
        for i in 0..5u8 {
            store.push(CaptureEntry {
                id: 0,
                ts_ms: 0,
                method: "GET".into(),
                url: format!("/{i}"),
                status: 200,
                duration_ms: 1,
                req_headers: Vec::new(),
                req_body: Vec::new(),
                resp_headers: Vec::new(),
                resp_body: Vec::new(),
            });
        }
        let snap = store.snapshot();
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[0].url, "/3");
        assert_eq!(snap[1].url, "/4");
    }
}
