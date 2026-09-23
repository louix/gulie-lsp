//! Cached access to the worker, shared by request handlers and background
//! analysis.

use crate::cache::LookupCache;
use crate::scope::Import;
use crate::syntax::datum::Datum;
use crate::worker::protocol::WorkerError;
use crate::worker::{Identity, Worker};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct Resolver {
    pub worker: Worker,
    pub cache: Arc<Mutex<LookupCache>>,
}

#[derive(Debug)]
pub enum Resolved {
    Found(Datum),
    /// The worker answered that the thing does not exist.
    NotFound,
    /// The worker is not up yet: answer from syntax alone.
    NotReady,
    /// The workspace is restricted; loading modules is refused.
    Restricted(String),
    Failed(WorkerError),
}

impl Resolver {
    pub fn identity(&self) -> Option<Identity> {
        self.worker.identity_if_ready()
    }

    /// A cached result, without asking the worker. Completion uses this so
    /// a keystroke never queues behind import warming.
    pub fn cached(&self, method: &str, params: &Datum) -> Option<Datum> {
        let identity = self.identity()?;
        let key = LookupCache::key(
            &identity.key,
            identity.generation,
            &Datum::list([Datum::symbol(method), params.clone()]),
        );
        self.cache.lock().unwrap().get(&key).cloned()
    }

    /// A worker request through the cache. Never waits for a starting worker.
    pub async fn request(&self, method: &str, params: Datum) -> Resolved {
        let Some(identity) = self.identity() else {
            return Resolved::NotReady;
        };
        let key = LookupCache::key(
            &identity.key,
            identity.generation,
            &Datum::list([Datum::symbol(method), params.clone()]),
        );
        if let Some(hit) = self.cache.lock().unwrap().get(&key).cloned() {
            return Resolved::Found(hit);
        }
        match self.worker.request(method, params).await {
            Ok(result) => {
                self.cache.lock().unwrap().insert(key, result.clone());
                Resolved::Found(result)
            }
            Err(WorkerError::Remote { kind, .. }) if kind == "not-found" => Resolved::NotFound,
            Err(WorkerError::Remote { kind, message }) if kind == "restricted" => {
                Resolved::Restricted(message)
            }
            Err(err) => Resolved::Failed(err),
        }
    }

    /// Resolve a local `name` through `imports`: one lookup per distinct
    /// exported name the imports could map it to. `Ok(None)` when no module
    /// binds it; `Err` carries the first non-not-found failure.
    pub async fn lookup(&self, imports: &[Import], name: &str) -> Result<Option<Datum>, Resolved> {
        for params in crate::semantic::lookup_params_for(imports, name) {
            match self.request("lookup", params).await {
                Resolved::Found(found) => return Ok(Some(found)),
                Resolved::NotFound => continue,
                other => return Err(other),
            }
        }
        Ok(None)
    }
}

pub fn exports_params(import: &Import) -> Datum {
    Datum::list([Datum::List(
        std::iter::once(Datum::symbol("module"))
            .chain(import.module.iter().map(|s| Datum::symbol(s)))
            .collect(),
    )])
}
