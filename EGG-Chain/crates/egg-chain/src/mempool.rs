#![forbid(unsafe_code)]

use std::collections::{HashMap, VecDeque};

use egg_crypto::{tx_id_from_payload, validate_tx_id};
use egg_types::{Hash256, Transaction};
use thiserror::Error;

const DEFAULT_MAX_TXS: usize = 100_000;
const DEFAULT_MAX_TOTAL_BYTES: usize = 64 * 1024 * 1024; // 64 MiB

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddOutcome {
    Added,
    AlreadyKnown,
}

#[derive(Debug, Error)]
pub enum MempoolError {
    #[error("invalid tx id: expected {expected:?}, got {got:?}")]
    InvalidTxId { expected: Hash256, got: Hash256 },

    #[error("tx payload too large: {size} bytes")]
    TxTooLarge { size: usize },

    #[error("mempool full")]
    Full,
}

pub type Result<T> = std::result::Result<T, MempoolError>;

#[derive(Clone)]
pub struct Mempool {
    by_id: HashMap<Hash256, Transaction>,
    order: VecDeque<Hash256>,
    total_payload_bytes: usize,
}

impl Mempool {
    pub fn new() -> Self {
        Self {
            by_id: HashMap::new(),
            order: VecDeque::new(),
            total_payload_bytes: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    pub fn total_payload_bytes(&self) -> usize {
        self.total_payload_bytes
    }

    pub fn contains(&self, txid: Hash256) -> bool {
        self.by_id.contains_key(&txid)
    }

    pub fn get(&self, txid: Hash256) -> Option<&Transaction> {
        self.by_id.get(&txid)
    }

    pub fn add_tx(&mut self, tx: Transaction) -> Result<AddOutcome> {
        let expected = tx_id_from_payload(&tx.payload);
        if tx.id != expected || !validate_tx_id(&tx) {
            return Err(MempoolError::InvalidTxId {
                expected,
                got: tx.id,
            });
        }

        if self.by_id.contains_key(&tx.id) {
            return Ok(AddOutcome::AlreadyKnown);
        }

        if tx.payload.len() > DEFAULT_MAX_TOTAL_BYTES {
            return Err(MempoolError::TxTooLarge {
                size: tx.payload.len(),
            });
        }

        if self.by_id.len() >= DEFAULT_MAX_TXS {
            return Err(MempoolError::Full);
        }

        if self
            .total_payload_bytes
            .saturating_add(tx.payload.len())
            > DEFAULT_MAX_TOTAL_BYTES
        {
            return Err(MempoolError::Full);
        }

        self.total_payload_bytes = self.total_payload_bytes.saturating_add(tx.payload.len());
        self.order.push_back(tx.id);
        self.by_id.insert(tx.id, tx);
        Ok(AddOutcome::Added)
    }

    pub fn remove(&mut self, txid: Hash256) -> Option<Transaction> {
        let tx = self.by_id.remove(&txid)?;
        self.total_payload_bytes = self.total_payload_bytes.saturating_sub(tx.payload.len());
        // giữ `order` đơn giản: không xoá giữa; sẽ được skip khi drain.
        Some(tx)
    }

    /// Lấy tối đa `max` tx theo thứ tự vào mempool (FIFO) và remove khỏi mempool.
    pub fn drain_fifo(&mut self, max: usize) -> Vec<Transaction> {
        let mut out = Vec::new();
        while out.len() < max {
            let Some(txid) = self.order.pop_front() else {
                break;
            };
            if let Some(tx) = self.by_id.remove(&txid) {
                self.total_payload_bytes = self.total_payload_bytes.saturating_sub(tx.payload.len());
                out.push(tx);
            }
        }
        out
    }
}

impl Default for Mempool {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egg_crypto::tx_id_from_payload;

    fn mk_tx(payload: &[u8]) -> Transaction {
        let id = tx_id_from_payload(payload);
        Transaction {
            id,
            payload: payload.to_vec(),
        }
    }

    #[test]
    fn add_and_dedup_by_txid() {
        let mut mp = Mempool::new();

        let tx1 = mk_tx(b"abc");
        let tx2 = mk_tx(b"abc"); // cùng payload => cùng txid

        assert_eq!(mp.add_tx(tx1).unwrap(), AddOutcome::Added);
        assert_eq!(mp.len(), 1);

        assert_eq!(mp.add_tx(tx2).unwrap(), AddOutcome::AlreadyKnown);
        assert_eq!(mp.len(), 1);
    }

    #[test]
    fn reject_invalid_txid() {
        let mut mp = Mempool::new();

        let mut tx = mk_tx(b"xyz");
        tx.id = Hash256([9u8; 32]); // sai

        let err = mp.add_tx(tx).unwrap_err();
        assert!(matches!(err, MempoolError::InvalidTxId { .. }));
        assert_eq!(mp.len(), 0);
    }

    #[test]
    fn remove_works() {
        let mut mp = Mempool::new();

        let tx = mk_tx(b"p");
        let id = tx.id;

        mp.add_tx(tx).unwrap();
        assert!(mp.contains(id));

        let got = mp.remove(id).expect("removed");
        assert_eq!(got.id, id);
        assert!(!mp.contains(id));
        assert_eq!(mp.len(), 0);
    }

    #[test]
    fn drain_fifo_returns_in_order() {
        let mut mp = Mempool::new();

        let a = mk_tx(b"a");
        let b = mk_tx(b"b");
        let c = mk_tx(b"c");

        mp.add_tx(a.clone()).unwrap();
        mp.add_tx(b.clone()).unwrap();
        mp.add_tx(c.clone()).unwrap();

        let out = mp.drain_fifo(2);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].id, a.id);
        assert_eq!(out[1].id, b.id);

        assert_eq!(mp.len(), 1);
        assert!(mp.contains(c.id));
    }
}
