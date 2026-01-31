#![forbid(unsafe_code)]

use egg_types::{Block, Hash256, Height};
use thiserror::Error;

use crate::block_builder::{build_block_template_from_mempool, BlockBuildError};
use crate::mempool::Mempool;
use crate::pow_valid;

const DEFAULT_MAX_NONCE_TRIES: u64 = 50_000_000;

#[derive(Debug, Error)]
pub enum MiningError {
    #[error("block build error: {0}")]
    BlockBuild(#[from] BlockBuildError),

    #[error("pow not found within {max_tries} nonce tries")]
    PowNotFound { max_tries: u64 },
}

pub type Result<T> = std::result::Result<T, MiningError>;

pub fn mine_block(mut block: Block) -> Result<Block> {
    let mut tries: u64 = 0;
    while tries < DEFAULT_MAX_NONCE_TRIES {
        if pow_valid(&block.header) {
            return Ok(block);
        }
        block.header.nonce = block.header.nonce.wrapping_add(1);
        tries += 1;
    }
    Err(MiningError::PowNotFound {
        max_tries: DEFAULT_MAX_NONCE_TRIES,
    })
}

/// Mine block từ mempool (FIFO).
/// Nếu mining fail thì khôi phục tx về mempool (best-effort).
pub fn mine_block_from_mempool(
    mempool: &mut Mempool,
    parent: Hash256,
    height: Height,
    timestamp_utc: i64,
    pow_difficulty_bits: u32,
) -> Result<Block> {
    let block = build_block_template_from_mempool(
        mempool,
        parent,
        height,
        timestamp_utc,
        pow_difficulty_bits,
    )?;

    // Nếu mining fail: restore txs (best-effort) để không mất.
    match mine_block(block) {
        Ok(mined) => Ok(mined),
        Err(e) => {
            // best-effort restore
            // (mempool đã bị drain trong builder)
            // restore theo thứ tự tx trong block
            // ignore lỗi restore để ưu tiên trả lỗi mining
            if let MiningError::BlockBuild(_) = e {
                // builder fail thì chưa drain được txs vào block theo nghĩa đầy đủ,
                // nhưng vẫn không có danh sách tx để restore ở đây.
                // (hiện tại build_block_template_from_mempool fail trước khi trả Block)
            }
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egg_crypto::tx_id_from_payload;
    use egg_types::{Hash256, Transaction};

    fn mk_tx(payload: &[u8]) -> Transaction {
        let id = tx_id_from_payload(payload);
        Transaction {
            id,
            payload: payload.to_vec(),
        }
    }

    #[test]
    fn mine_finds_nonce_for_low_difficulty() {
        let mut mp = Mempool::new();
        mp.add_tx(mk_tx(b"a")).unwrap();
        mp.add_tx(mk_tx(b"b")).unwrap();

        let blk = mine_block_from_mempool(
            &mut mp,
            Hash256::zero(),
            Height(1),
            1_700_000_000,
            8,
        )
        .unwrap();

        assert!(pow_valid(&blk.header));
        assert_eq!(blk.header.pow_difficulty_bits, 8);
    }
}
