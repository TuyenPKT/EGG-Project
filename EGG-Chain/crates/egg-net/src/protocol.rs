#![forbid(unsafe_code)]

use egg_types::{canonical, Block, BlockHeader, Hash256};

const MAGIC: [u8; 8] = *b"EGGNET00";
const VERSION: u16 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tip {
    pub height: u64,
    pub hash: Hash256,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    // handshake
    Hello {
        chain_id: u32,
        genesis_id: Hash256,
        tip: Tip,
        node_nonce: u64,
        agent: String,
    },
    HelloAck {
        chain_id: u32,
        genesis_id: Hash256,
        tip: Tip,
        node_nonce: u64,
        agent: String,
    },

    // headers-first
    GetHeaders { start: Hash256, max: u32 },
    Headers { headers: Vec<BlockHeader> },

    // block download
    GetBlock { id: Hash256 },
    Block { id: Hash256, block: Option<Block> },

    // keepalive
    Ping { nonce: u64 },
    Pong { nonce: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    UnexpectedEof { at: usize, needed: usize, remaining: usize },
    InvalidMagic { at: usize },
    UnsupportedVersion { got: u16 },
    InvalidTag { tag: u8 },
    LengthOverflow { at: usize },
    InvalidUtf8 { at: usize },
    Canonical(String),
}

impl core::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ProtocolError::UnexpectedEof { at, needed, remaining } => write!(
                f,
                "unexpected eof at {} (needed {}, remaining {})",
                at, needed, remaining
            ),
            ProtocolError::InvalidMagic { at } => write!(f, "invalid magic at {}", at),
            ProtocolError::UnsupportedVersion { got } => write!(f, "unsupported version {}", got),
            ProtocolError::InvalidTag { tag } => write!(f, "invalid message tag {}", tag),
            ProtocolError::LengthOverflow { at } => write!(f, "length overflow at {}", at),
            ProtocolError::InvalidUtf8 { at } => write!(f, "invalid utf8 at {}", at),
            ProtocolError::Canonical(e) => write!(f, "canonical decode error: {}", e),
        }
    }
}

impl std::error::Error for ProtocolError {}

type Result<T> = core::result::Result<T, ProtocolError>;

struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let rem = self.remaining();
        if rem < n {
            return Err(ProtocolError::UnexpectedEof {
                at: self.pos,
                needed: n,
                remaining: rem,
            });
        }
        let out = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }

    fn take_u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn take_u16_be(&mut self) -> Result<u16> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    fn take_u32_be(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn take_u64_be(&mut self) -> Result<u64> {
        let b = self.take(8)?;
        Ok(u64::from_be_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    fn take_hash256(&mut self) -> Result<Hash256> {
        let b = self.take(32)?;
        let mut out = [0u8; 32];
        out.copy_from_slice(b);
        Ok(Hash256(out))
    }

    fn expect_magic(&mut self) -> Result<()> {
        let at = self.pos;
        let b = self.take(8)?;
        if b != MAGIC {
            return Err(ProtocolError::InvalidMagic { at });
        }
        Ok(())
    }

    fn take_string_len_u32(&mut self) -> Result<String> {
        let at = self.pos;
        let len = self.take_u32_be()? as usize;
        let bytes = self.take(len)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| ProtocolError::InvalidUtf8 { at })
    }

    fn take_bytes_len_u32(&mut self) -> Result<Vec<u8>> {
        let len = self.take_u32_be()? as usize;
        Ok(self.take(len)?.to_vec())
    }
}

fn push_u8(out: &mut Vec<u8>, v: u8) {
    out.push(v);
}
fn push_u16_be(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_be_bytes());
}
fn push_u32_be(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_be_bytes());
}
fn push_u64_be(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_be_bytes());
}
fn push_hash256(out: &mut Vec<u8>, h: Hash256) {
    out.extend_from_slice(&h.0);
}
fn push_string_len_u32(out: &mut Vec<u8>, s: &str) -> Result<()> {
    let len: u32 = s
        .as_bytes()
        .len()
        .try_into()
        .map_err(|_| ProtocolError::LengthOverflow { at: out.len() })?;
    push_u32_be(out, len);
    out.extend_from_slice(s.as_bytes());
    Ok(())
}
fn push_bytes_len_u32(out: &mut Vec<u8>, b: &[u8]) -> Result<()> {
    let len: u32 = b
        .len()
        .try_into()
        .map_err(|_| ProtocolError::LengthOverflow { at: out.len() })?;
    push_u32_be(out, len);
    out.extend_from_slice(b);
    Ok(())
}

fn encode_tip(out: &mut Vec<u8>, tip: Tip) {
    push_u64_be(out, tip.height);
    push_hash256(out, tip.hash);
}

fn decode_tip(c: &mut Cursor<'_>) -> Result<Tip> {
    let height = c.take_u64_be()?;
    let hash = c.take_hash256()?;
    Ok(Tip { height, hash })
}

// Tags
const TAG_HELLO: u8 = 1;
const TAG_HELLO_ACK: u8 = 2;

const TAG_GET_HEADERS: u8 = 10;
const TAG_HEADERS: u8 = 11;

const TAG_GET_BLOCK: u8 = 12;
const TAG_BLOCK: u8 = 13;

const TAG_PING: u8 = 20;
const TAG_PONG: u8 = 21;

/// Binary encoding:
/// MAGIC(8) + VERSION(u16) + TAG(u8) + payload...
pub fn encode_message(msg: &Message) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    out.extend_from_slice(&MAGIC);
    push_u16_be(&mut out, VERSION);

    match msg {
        Message::Hello {
            chain_id,
            genesis_id,
            tip,
            node_nonce,
            agent,
        } => {
            push_u8(&mut out, TAG_HELLO);
            push_u32_be(&mut out, *chain_id);
            push_hash256(&mut out, *genesis_id);
            encode_tip(&mut out, *tip);
            push_u64_be(&mut out, *node_nonce);
            push_string_len_u32(&mut out, agent)?;
        }
        Message::HelloAck {
            chain_id,
            genesis_id,
            tip,
            node_nonce,
            agent,
        } => {
            push_u8(&mut out, TAG_HELLO_ACK);
            push_u32_be(&mut out, *chain_id);
            push_hash256(&mut out, *genesis_id);
            encode_tip(&mut out, *tip);
            push_u64_be(&mut out, *node_nonce);
            push_string_len_u32(&mut out, agent)?;
        }
        Message::GetHeaders { start, max } => {
            push_u8(&mut out, TAG_GET_HEADERS);
            push_hash256(&mut out, *start);
            push_u32_be(&mut out, *max);
        }
        Message::Headers { headers } => {
            push_u8(&mut out, TAG_HEADERS);
            let n: u32 = headers.len().try_into().unwrap_or(u32::MAX);
            push_u32_be(&mut out, n);

            for h in headers {
                let hb = canonical::encode_block_header(h);
                push_bytes_len_u32(&mut out, &hb)?;
            }
        }
        Message::GetBlock { id } => {
            push_u8(&mut out, TAG_GET_BLOCK);
            push_hash256(&mut out, *id);
        }
        Message::Block { id, block } => {
            push_u8(&mut out, TAG_BLOCK);
            push_hash256(&mut out, *id);

            match block {
                None => {
                    push_u8(&mut out, 0);
                }
                Some(b) => {
                    push_u8(&mut out, 1);
                    let bb = canonical::encode_block(b);
                    push_bytes_len_u32(&mut out, &bb)?;
                }
            }
        }
        Message::Ping { nonce } => {
            push_u8(&mut out, TAG_PING);
            push_u64_be(&mut out, *nonce);
        }
        Message::Pong { nonce } => {
            push_u8(&mut out, TAG_PONG);
            push_u64_be(&mut out, *nonce);
        }
    }

    Ok(out)
}

pub fn decode_message(bytes: &[u8]) -> Result<Message> {
    let mut c = Cursor::new(bytes);
    c.expect_magic()?;
    let ver = c.take_u16_be()?;
    if ver != VERSION {
        return Err(ProtocolError::UnsupportedVersion { got: ver });
    }

    let tag = c.take_u8()?;
    match tag {
        TAG_HELLO => {
            let chain_id = c.take_u32_be()?;
            let genesis_id = c.take_hash256()?;
            let tip = decode_tip(&mut c)?;
            let node_nonce = c.take_u64_be()?;
            let agent = c.take_string_len_u32()?;
            Ok(Message::Hello {
                chain_id,
                genesis_id,
                tip,
                node_nonce,
                agent,
            })
        }
        TAG_HELLO_ACK => {
            let chain_id = c.take_u32_be()?;
            let genesis_id = c.take_hash256()?;
            let tip = decode_tip(&mut c)?;
            let node_nonce = c.take_u64_be()?;
            let agent = c.take_string_len_u32()?;
            Ok(Message::HelloAck {
                chain_id,
                genesis_id,
                tip,
                node_nonce,
                agent,
            })
        }
        TAG_GET_HEADERS => {
            let start = c.take_hash256()?;
            let max = c.take_u32_be()?;
            Ok(Message::GetHeaders { start, max })
        }
        TAG_HEADERS => {
            let n = c.take_u32_be()? as usize;
            let mut headers = Vec::with_capacity(n);
            for _ in 0..n {
                let hb = c.take_bytes_len_u32()?;
                let h = canonical::decode_block_header(&hb)
                    .map_err(|e| ProtocolError::Canonical(e.to_string()))?;
                headers.push(h);
            }
            Ok(Message::Headers { headers })
        }
        TAG_GET_BLOCK => {
            let id = c.take_hash256()?;
            Ok(Message::GetBlock { id })
        }
        TAG_BLOCK => {
            let id = c.take_hash256()?;
            let flag = c.take_u8()?;
            match flag {
                0 => Ok(Message::Block { id, block: None }),
                1 => {
                    let bb = c.take_bytes_len_u32()?;
                    let b = canonical::decode_block(&bb)
                        .map_err(|e| ProtocolError::Canonical(e.to_string()))?;
                    Ok(Message::Block { id, block: Some(b) })
                }
                other => Err(ProtocolError::InvalidTag { tag: other }),
            }
        }
        TAG_PING => {
            let nonce = c.take_u64_be()?;
            Ok(Message::Ping { nonce })
        }
        TAG_PONG => {
            let nonce = c.take_u64_be()?;
            Ok(Message::Pong { nonce })
        }
        other => Err(ProtocolError::InvalidTag { tag: other }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egg_types::{Hash256, Height};

    fn sample_header(nonce: u64, height: u64) -> BlockHeader {
        BlockHeader {
            parent: Hash256([1u8; 32]),
            height: Height(height),
            timestamp_utc: 1_700_000_000,
            nonce,
            merkle_root: Hash256([2u8; 32]),
            pow_difficulty_bits: 8,
        }
    }

    #[test]
    fn roundtrip_hello() {
        let m = Message::Hello {
            chain_id: 1,
            genesis_id: Hash256([9u8; 32]),
            tip: Tip {
                height: 7,
                hash: Hash256([8u8; 32]),
            },
            node_nonce: 123,
            agent: "egg-node/0.1".to_string(),
        };

        let enc = encode_message(&m).unwrap();
        let dec = decode_message(&enc).unwrap();
        assert_eq!(m, dec);
    }

    #[test]
    fn roundtrip_headers() {
        let m = Message::Headers {
            headers: vec![sample_header(1, 1), sample_header(2, 2)],
        };
        let enc = encode_message(&m).unwrap();
        let dec = decode_message(&enc).unwrap();
        assert_eq!(m, dec);
    }

    #[test]
    fn roundtrip_block_empty_txs_header_matches() {
        let blk = Block {
            header: sample_header(7, 3),
            txs: vec![],
        };
        let m = Message::Block {
            id: Hash256([3u8; 32]),
            block: Some(blk.clone()),
        };

        let enc = encode_message(&m).unwrap();
        let dec = decode_message(&enc).unwrap();

        let Message::Block { id, block } = dec else { panic!("expected Block"); };
        assert_eq!(id, Hash256([3u8; 32]));
        let b = block.expect("expected Some(block)");
        assert_eq!(b.header, blk.header);
        assert_eq!(b.txs.len(), 0);
    }
}
