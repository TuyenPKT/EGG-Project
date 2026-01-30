#![forbid(unsafe_code)]

use egg_chain::{header_id, pow_valid};
use egg_types::{BlockHeader, Hash256, Height};

#[test]
fn smoke_chain_header_hash_and_pow() {
    let h = BlockHeader {
        parent: Hash256::zero(),
        height: Height(0),
        timestamp_utc: 1_700_000_000,
        nonce: 0,
        merkle_root: Hash256::zero(),
        pow_difficulty_bits: 0,
    };

    let id = header_id(&h);
    assert_ne!(id, Hash256::zero());

    // difficulty_bits = 0 => luôn hợp lệ
    assert!(pow_valid(&h));
}
