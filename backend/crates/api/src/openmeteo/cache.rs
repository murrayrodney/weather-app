//! TTL cache with per-key single flight, stale-if-error and a short negative cache (02 5.6).

use std::collections::HashMap;
use std::future::Future;
use std::hash::Hash;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, Utc};
use famweather_core::clock::Clock;

use super::OmError;

/// A failed upstream call is remembered this long, so requests that queued behind it (and the next callers) get the
/// stale entry or the same error at once instead of each waiting out its own timeout.
pub const ERROR_TTL: Duration = Duration::seconds(10);

struct Slot<V> {
    value: Option<(Arc<V>, DateTime<Utc>)>,
    error: Option<(OmError, DateTime<Utc>)>,
    gate: Arc<tokio::sync::Mutex<()>>,
}

pub struct TtlCache<K, V> {
    map: Mutex<HashMap<K, Slot<V>>>,
    ttl: Duration,
    stale_max: Duration,
    max_entries: usize,
    clock: Arc<dyn Clock>,
}

/// What a lookup returned: the value, when it was fetched, and whether it is older than the TTL (a failed refresh).
pub struct Cached<V> {
    pub value: Arc<V>,
    pub fetched_at: DateTime<Utc>,
    pub stale: bool,
}

/// A copy of one slot's state.
struct View<V> {
    value: Option<(Arc<V>, DateTime<Utc>)>,
    error: Option<(OmError, DateTime<Utc>)>,
    gate: Arc<tokio::sync::Mutex<()>>,
}

impl<K: Hash + Eq + Clone, V> TtlCache<K, V> {
    pub fn new(
        ttl: Duration,
        stale_max: Duration,
        max_entries: usize,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            map: Mutex::new(HashMap::new()),
            ttl,
            stale_max,
            max_entries,
            clock,
        }
    }

    pub fn len(&self) -> usize {
        self.map.lock().unwrap_or_else(|p| p.into_inner()).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn view(&self, key: &K) -> View<V> {
        let mut g = self.map.lock().unwrap_or_else(|p| p.into_inner());
        if !g.contains_key(key) && g.len() >= self.max_entries {
            // Evict the entry fetched longest ago, but NEVER a slot someone is fetching or waiting on: an in-flight
            // gate has more than one owner. If every slot is busy the map simply grows past its bound for a moment.
            let oldest = g
                .iter()
                .filter(|(_, s)| Arc::strong_count(&s.gate) == 1)
                .min_by_key(|(_, s)| s.value.as_ref().map(|v| v.1))
                .map(|(k, _)| k.clone());
            if let Some(k) = oldest {
                g.remove(&k);
            }
        }
        let slot = g.entry(key.clone()).or_insert_with(|| Slot {
            value: None,
            error: None,
            gate: Arc::new(tokio::sync::Mutex::new(())),
        });
        View {
            value: slot.value.clone(),
            error: slot.error,
            gate: slot.gate.clone(),
        }
    }

    fn fresh(&self, v: &View<V>) -> Option<Cached<V>> {
        v.value
            .as_ref()
            .filter(|(_, at)| self.clock.now() - *at < self.ttl)
            .map(|(value, at)| Cached {
                value: value.clone(),
                fetched_at: *at,
                stale: false,
            })
    }

    /// The stale entry (if still young enough) or the error.
    fn fallback(&self, v: View<V>, err: OmError) -> Result<Cached<V>, OmError> {
        match v.value {
            Some((value, at)) if self.clock.now() - at <= self.stale_max => Ok(Cached {
                value,
                fetched_at: at,
                stale: true,
            }),
            _ => Err(err),
        }
    }

    /// Concurrent misses for one key wait on a single fetch.
    pub async fn get<F, Fut>(&self, key: K, fetch: F) -> Result<Cached<V>, OmError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<V, OmError>>,
    {
        let first = self.view(&key);
        if let Some(c) = self.fresh(&first) {
            return Ok(c);
        }
        let gate = first.gate.clone();
        drop(first);
        let _flight = gate.lock().await;
        let view = self.view(&key);
        if let Some(c) = self.fresh(&view) {
            return Ok(c);
        }
        if let Some((err, at)) = view.error {
            if self.clock.now() - at < ERROR_TTL {
                return self.fallback(view, err);
            }
        }
        match fetch().await {
            Ok(v) => {
                let at = self.clock.now();
                let v = Arc::new(v);
                let mut g = self.map.lock().unwrap_or_else(|p| p.into_inner());
                if let Some(s) = g.get_mut(&key) {
                    s.value = Some((v.clone(), at));
                    s.error = None;
                }
                Ok(Cached {
                    value: v,
                    fetched_at: at,
                    stale: false,
                })
            }
            Err(e) => {
                let mut g = self.map.lock().unwrap_or_else(|p| p.into_inner());
                if let Some(s) = g.get_mut(&key) {
                    s.error = Some((e, self.clock.now()));
                }
                drop(g);
                self.fallback(view, e)
            }
        }
    }
}
