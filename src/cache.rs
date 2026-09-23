//! Bounded cache for worker lookups. Keys include the environment identity
//! and the worker generation, so a switched environment or a restarted
//! worker never serves stale metadata; saving a file clears everything,
//! because the worker reads modules from disk.

use crate::syntax::datum::Datum;
use std::collections::{HashMap, VecDeque};

pub struct LookupCache {
    capacity: usize,
    entries: HashMap<String, Datum>,
    order: VecDeque<String>,
}

impl LookupCache {
    pub fn new(capacity: usize) -> Self {
        LookupCache {
            capacity,
            entries: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    pub fn key(identity: &str, generation: u64, params: &Datum) -> String {
        format!("{identity}#{generation}:{params}")
    }

    pub fn get(&self, key: &str) -> Option<&Datum> {
        self.entries.get(key)
    }

    pub fn insert(&mut self, key: String, value: Datum) {
        if self.entries.insert(key.clone(), value).is_none() {
            self.order.push_back(key);
            while self.order.len() > self.capacity {
                if let Some(oldest) = self.order.pop_front() {
                    self.entries.remove(&oldest);
                }
            }
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evicts_oldest_and_distinguishes_generations() {
        let mut cache = LookupCache::new(2);
        let params = Datum::parse_one("((name . fold))").unwrap();
        let k1 = LookupCache::key("guile-3.0.10", 1, &params);
        let k2 = LookupCache::key("guile-3.0.10", 2, &params);
        assert_ne!(k1, k2);
        cache.insert(k1.clone(), Datum::Integer(1));
        cache.insert(k2.clone(), Datum::Integer(2));
        cache.insert("third".into(), Datum::Integer(3));
        assert!(cache.get(&k1).is_none(), "oldest evicted");
        assert_eq!(cache.get(&k2), Some(&Datum::Integer(2)));
        assert_eq!(cache.len(), 2);
        cache.clear();
        assert!(cache.is_empty());
    }
}
