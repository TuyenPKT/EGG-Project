#![forbid(unsafe_code)]

use std::path::Path;

use crate::{DbError, KvStore, Result};

#[derive(Clone)]
pub struct SledKv {
    db: sled::Db,
}

impl SledKv {
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let db = sled::open(path)?;
        Ok(Self { db })
    }
}

impl KvStore for SledKv {
    fn get(&self, key: &[u8]) -> Result<Vec<u8>> {
        match self.db.get(key)? {
            Some(v) => Ok(v.to_vec()),
            None => Err(DbError::NotFound),
        }
    }

    fn put(&self, key: Vec<u8>, value: Vec<u8>) -> Result<()> {
        self.db.insert(key, value)?;
        self.db.flush()?;
        Ok(())
    }

    fn del(&self, key: &[u8]) -> Result<()> {
        let _ = self.db.remove(key)?;
        self.db.flush()?;
        Ok(())
    }

    fn has(&self, key: &[u8]) -> Result<bool> {
        Ok(self.db.contains_key(key)?)
    }
}
