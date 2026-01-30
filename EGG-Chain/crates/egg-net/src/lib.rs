#![forbid(unsafe_code)]

use egg_types::{BlockHeader, Hash256};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NetMsg {
    Ping { nonce: u64 },
    Pong { nonce: u64 },
    GetHeaders { from: Hash256, limit: u32 },
    Headers { headers: Vec<BlockHeader> },
}

pub fn encode(msg: &NetMsg) -> Vec<u8> {
    bincode::serialize(msg).expect("bincode encode")
}

pub fn decode(bytes: &[u8]) -> NetMsg {
    bincode::deserialize(bytes).expect("bincode decode")
}

#[cfg(test)]
mod tests {
    use super::*;
    use egg_types::{Height, Hash256};

    #[test]
    fn roundtrip_ping() {
        let m = NetMsg::Ping { nonce: 123 };
        let b = encode(&m);
        let d = decode(&b);
        assert_eq!(m, d);
    }

    #[test]
    fn roundtrip_headers() {
        let h = BlockHeader {
            parent: Hash256::zero(),
            height: Height(1),
            timestamp_utc: 1_700_000_000,
            nonce: 7,
            merkle_root: Hash256::zero(),
            pow_difficulty_bits: 8,
        };
        let m = NetMsg::Headers { headers: vec![h] };
        let b = encode(&m);
        let d = decode(&b);
        assert_eq!(m, d);
    }
}
