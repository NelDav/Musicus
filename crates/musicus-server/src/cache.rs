//! Keeps recently served sheet files in RAM, so the burst of a whole
//! orchestra opening the same piece is answered without touching the disk.
//! `Bytes` is reference counted: every client shares one copy.

use bytes::Bytes;
use std::{collections::HashMap, sync::Mutex};

pub struct FileCache {
    capacity: u64,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    entries: HashMap<String, (Bytes, u64)>,
    total: u64,
    clock: u64,
}

impl FileCache {
    pub fn new(capacity_bytes: u64) -> Self {
        Self { capacity: capacity_bytes, inner: Mutex::default() }
    }

    pub fn get(&self, key: &str) -> Option<Bytes> {
        let mut inner = self.inner.lock().expect("cache lock");
        inner.clock += 1;
        let clock = inner.clock;
        let entry = inner.entries.get_mut(key)?;
        entry.1 = clock;
        Some(entry.0.clone())
    }

    /// Inserts, evicting the least recently used entries. Files larger than
    /// a quarter of the capacity are not cached.
    pub fn insert(&self, key: &str, bytes: Bytes) {
        let size = bytes.len() as u64;
        if size > self.capacity / 4 {
            return;
        }
        let mut inner = self.inner.lock().expect("cache lock");
        if inner.entries.contains_key(key) {
            return;
        }
        while inner.total + size > self.capacity {
            let Some(oldest) = inner.entries.iter().min_by_key(|(_, (_, used))| *used).map(|(k, _)| k.clone()) else {
                break;
            };
            if let Some((evicted, _)) = inner.entries.remove(&oldest) {
                inner.total -= evicted.len() as u64;
            }
        }
        inner.clock += 1;
        let clock = inner.clock;
        inner.total += size;
        inner.entries.insert(key.to_owned(), (bytes, clock));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evicts_least_recently_used() {
        let cache = FileCache::new(40);
        cache.insert("a", Bytes::from(vec![0; 10]));
        cache.insert("b", Bytes::from(vec![0; 10]));
        cache.insert("c", Bytes::from(vec![0; 10]));
        assert!(cache.get("a").is_some()); // "b" is now the oldest
        cache.insert("d", Bytes::from(vec![0; 10]));
        cache.insert("e", Bytes::from(vec![0; 10]));
        assert!(cache.get("b").is_none());
        assert!(cache.get("a").is_some() && cache.get("e").is_some());
        cache.insert("huge", Bytes::from(vec![0; 11]));
        assert!(cache.get("huge").is_none());
    }
}
