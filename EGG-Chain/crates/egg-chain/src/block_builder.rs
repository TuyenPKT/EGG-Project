#![forbid(unsafe_code)]

use egg_crypto::{merkle::merkle_root_txids, tx_id_from_payload, validate_tx_id};
use egg_types::{Block, BlockHeader, Hash256, Height, Transaction};
use thiserror::Error;

use crate::mempool::Mempool;

const MAX_TXS_PER_BLOCK: usize = 10_000;

#[derive(Debug, Error)]
pub enum BlockBuildError {
    #[error("invalid tx id at index {index}: expected {expected:?}, got {got:?}")]
    InvalidTxId {
        index: usize,
        expected: Hash256,
        got: Hash256,
    },

    #[error("merkle mismatch: expected {expected:?}, got {got:?}")]
    MerkleMismatch { expected: Hash256, got: Hash256 },
}

pub type Result<T> = std::result::Result<T, BlockBuildError>;

pub fn compute_merkle_root_from_txs(txs: &[Transaction]) -> Result<Hash256> {
    for (i, tx) in txs.iter().enumerate() {
        if !validate_tx_id(tx) {
            let expected = tx_id_from_payload(&tx.payload);
            return Err(BlockBuildError::InvalidTxId {
                index: i,
                expected,
                got: tx.id,
            });
        }
    }
    let leaves: Vec<Hash256> = txs.iter().map(|t| t.id).collect();
    Ok(merkle_root_txids(&leaves))
}

pub fn verify_block_merkle(block: &Block) -> Result<()> {
    let expected = compute_merkle_root_from_txs(&block.txs)?;
    if block.header.merkle_root != expected {
        return Err(BlockBuildError::MerkleMismatch {
            expected,
            got: block.header.merkle_root,
        });
    }
    Ok(())
}

/// Build block template từ mempool (FIFO), set merkle_root đúng chuẩn.
/// Nonce mặc định = 0 (mining xử lý ở bước sau).
pub fn build_block_template_from_mempool(
    mempool: &mut Mempool,
    parent: Hash256,
    height: Height,
    timestamp_utc: i64,
    pow_difficulty_bits: u32,
) -> Result<Block> {
    let txs = mempool.drain_fifo(MAX_TXS_PER_BLOCK);
    let merkle_root = compute_merkle_root_from_txs(&txs)?;

    let header = BlockHeader {
        parent,
        height,
        timestamp_utc,
        nonce: 0,
        merkle_root,
        pow_difficulty_bits,
    };

    Ok(Block { header, txs })
}

#[cfg(test)]
mod tests {
    use super::*;
    use egg_crypto::tx_id_from_payload;
    use egg_types::Hash256;

    fn mk_tx(payload: &[u8]) -> Transaction {
        let id = tx_id_from_payload(payload);
        Transaction { id, payload: payload.to_vec() }
    }

    #[test]
    fn verify_block_merkle_ok_and_detects_tamper() {
        let mut mp = Mempool::new();
        mp.add_tx(mk_tx(b"a")).unwrap();
        mp.add_tx(mk_tx(b"b")).unwrap();
        mp.add_tx(mk_tx(b"c")).unwrap();

        let parent = Hash256([7u8; 32]);
        let blk = build_block_template_from_mempool(&mut mp, parent, Height(1), 1_700_000_000, 0).unwrap();

        // đúng
        verify_block_merkle(&blk).unwrap();

        // tamper header root
        let mut bad = blk.clone();
        bad.header.merkle_root = Hash256([9u8; 32]);
        let err = verify_block_merkle(&bad).unwrap_err();
        assert!(matches!(err, BlockBuildError::MerkleMismatch { .. }));
    }

    #[test]
    fn compute_merkle_rejects_invalid_txid() {
        let mut tx = mk_tx(b"x");
        tx.id = Hash256([1u8; 32]); // sai
        let err = compute_merkle_root_from_txs(&[tx]).unwrap_err();
        assert!(matches!(err, BlockBuildError::InvalidTxId { .. }));
    }

    #[test]
    fn fifo_order_preserved_from_mempool() {
        let mut mp = Mempool::new();
        let a = mk_tx(b"a");
        let b = mk_tx(b"b");
        mp.add_tx(a.clone()).unwrap();
        mp.add_tx(b.clone()).unwrap();

        let blk = build_block_template_from_mempool(
            &mut mp,
            Hash256::zero(),
            Height(1),
            1_700_000_000,
            0,
        )
        .unwrap();

        assert_eq!(blk.txs.len(), 2);
        assert_eq!(blk.txs[0].id, a.id);
        assert_eq!(blk.txs[1].id, b.id);

        // mempool drained
        assert_eq!(mp.len(), 0);
    }
}
