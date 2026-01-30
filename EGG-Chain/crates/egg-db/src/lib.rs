#![forbid(unsafe_code)]

use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};

use thiserror::Error;

pub mod sled_kv;
pub mod store;

pub use sled_kv::SledKv;

#[derive(Debug, Error)]
pub enum DbError {
    #[error("key not found")]
    NotFound,

    #[error("sled error: {0}")]
    Sled(#[from] sled::Error),
}

pub type Result<T> = std::result::Result<T, DbError>;

pub trait KvStore: Send + Sync + 'static {
    fn get(&self, key: &[u8]) -> Result<Vec<u8>>;
    fn put(&self, key: Vec<u8>, value: Vec<u8>) -> Result<()>;
    fn del(&self, key: &[u8]) -> Result<()>;
    fn has(&self, key: &[u8]) -> Result<bool>;
}

#[derive(Clone, Default)]
pub struct MemKv {
    inner: Arc<RwLock<HashMap<Vec<u8>, Vec<u8>>>>,
}

impl MemKv {
    pub fn new() -> Self {
        Self::default()
    }
}

impl KvStore for MemKv {
    fn get(&self, key: &[u8]) -> Result<Vec<u8>> {
        let g = self.inner.read().expect("rwlock poisoned");
        g.get(key).cloned().ok_or(DbError::NotFound)
    }

    fn put(&self, key: Vec<u8>, value: Vec<u8>) -> Result<()> {
        let mut g = self.inner.write().expect("rwlock poisoned");
        g.insert(key, value);
        Ok(())
    }

    fn del(&self, key: &[u8]) -> Result<()> {
        let mut g = self.inner.write().expect("rwlock poisoned");
        g.remove(key);
        Ok(())
    }

    fn has(&self, key: &[u8]) -> Result<bool> {
        let g = self.inner.read().expect("rwlock poisoned");
        Ok(g.contains_key(key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memkv_put_get_del() {
        let db = MemKv::new();

        assert_eq!(db.has(b"a").unwrap(), false);
        assert!(matches!(db.get(b"a"), Err(DbError::NotFound)));

        db.put(b"a".to_vec(), b"1".to_vec()).unwrap();
        assert_eq!(db.has(b"a").unwrap(), true);
        assert_eq!(db.get(b"a").unwrap(), b"1".to_vec());

        db.del(b"a").unwrap();
        assert_eq!(db.has(b"a").unwrap(), false);
        assert!(matches!(db.get(b"a"), Err(DbError::NotFound)));
    }
}
