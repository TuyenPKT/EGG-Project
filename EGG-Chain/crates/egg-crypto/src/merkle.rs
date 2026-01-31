#![forbid(unsafe_code)]

use egg_types::Hash256;

use crate::{hash_domain, DOMAIN_MERKLE};

fn merkle_parent(left: Hash256, right: Hash256) -> Hash256 {
    let mut buf = [0u8; 64];
    buf[0..32].copy_from_slice(&left.0);
    buf[32..64].copy_from_slice(&right.0);
    hash_domain(DOMAIN_MERKLE, &buf)
}

/// Merkle root deterministic cho danh sách TxID.
/// - Nếu danh sách rỗng => Hash256::zero()
/// - Nếu số lá lẻ => duplicate lá cuối
pub fn merkle_root_txids(txids: &[Hash256]) -> Hash256 {
    if txids.is_empty() {
        return Hash256::zero();
    }

    let mut layer: Vec<Hash256> = txids.to_vec();
    while layer.len() > 1 {
        let mut next = Vec::with_capacity((layer.len() + 1) / 2);
        for pair in layer.chunks(2) {
            let l = pair[0];
            let r = if pair.len() == 2 { pair[1] } else { pair[0] };
            next.push(merkle_parent(l, r));
        }
        layer = next;
    }
    layer[0]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(b: u8) -> Hash256 {
        Hash256([b; 32])
    }

    #[test]
    fn empty_is_zero() {
        assert_eq!(merkle_root_txids(&[]), Hash256::zero());
    }

    #[test]
    fn single_is_itself() {
        let a = h(1);
        assert_eq!(merkle_root_txids(&[a]), a);
    }

    #[test]
    fn order_changes_root() {
        let a = h(1);
        let b = h(2);
        let c = h(3);

        let r1 = merkle_root_txids(&[a, b, c]);
        let r2 = merkle_root_txids(&[c, b, a]);
        assert_ne!(r1, r2);
    }

    #[test]
    fn deterministic() {
        let a = h(9);
        let b = h(10);
        let r1 = merkle_root_txids(&[a, b]);
        let r2 = merkle_root_txids(&[a, b]);
        assert_eq!(r1, r2);
    }
}
