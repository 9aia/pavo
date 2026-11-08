//! A small TTL cache — `Mutex<HashMap>` + `Instant` expiries, std-only.
//!
//! Used by [`crate::host::Host`] for view-model results; renderers and
//! CLI verbs can also hold one for repeated `resolve_view` calls. Keys
//! are caller-chosen strings; values must be `Clone` (hits clone out —
//! the cache never hands out references into the lock).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

struct Entry<V> {
    value: V,
    expires: Instant,
}

/// TTL cache over string keys.
pub struct TtlCache<V> {
    ttl: Duration,
    inner: Mutex<HashMap<String, Entry<V>>>,
}

impl<V: Clone> TtlCache<V> {
    /// Empty cache; entries live `ttl`.
    #[must_use]
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            inner: Mutex::new(HashMap::new()),
        }
    }

    /// The TTL entries are inserted with.
    #[must_use]
    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    /// Clone the value for `key` if present and unexpired. Expired
    /// entries are evicted lazily on access.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<V> {
        let mut inner = self.inner.lock().ok()?;
        match inner.get(key) {
            Some(entry) if Instant::now() < entry.expires => Some(entry.value.clone()),
            Some(_) => {
                inner.remove(key);
                None
            }
            None => None,
        }
    }

    /// Insert or replace `key` → `value`, expiring after the TTL.
    pub fn insert(&self, key: impl Into<String>, value: V) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.insert(
                key.into(),
                Entry {
                    value,
                    expires: Instant::now() + self.ttl,
                },
            );
        }
    }

    /// Drop one entry.
    pub fn invalidate(&self, key: &str) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.remove(key);
        }
    }

    /// Drop every key starting with `prefix` — e.g. all cached
    /// `driver:view␟params` entries for one view after a `submit`
    /// mutates its backing state.
    pub fn invalidate_prefix(&self, prefix: &str) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.retain(|k, _| !k.starts_with(prefix));
        }
    }

    /// Drop everything.
    pub fn clear(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.clear();
        }
    }

    /// Number of live entries (expired ones count until touched —
    /// cheap enough for diagnostics, not for hot paths).
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.lock().map(|i| i.len()).unwrap_or(0)
    }

    /// Whether the cache holds no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn get_insert_invalidate() {
        let c = TtlCache::new(Duration::from_secs(60));
        assert!(c.is_empty());
        c.insert("k", 42);
        assert_eq!(c.get("k"), Some(42));
        c.invalidate("k");
        assert_eq!(c.get("k"), None);
        c.insert("a", 1);
        c.insert("b", 2);
        assert_eq!(c.len(), 2);
        c.clear();
        assert!(c.is_empty());
    }

    #[test]
    fn zero_ttl_expires_immediately() {
        let c = TtlCache::new(Duration::ZERO);
        c.insert("k", "v");
        assert_eq!(c.get("k"), None);
    }
}
