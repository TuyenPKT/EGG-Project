#![forbid(unsafe_code)]

use crate::protocol::{decode_message, encode_message, Message, ProtocolError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameError {
    TooLarge { len: u32 },
    Protocol(ProtocolError),
    UnexpectedEof { needed: usize, remaining: usize },
}

impl core::fmt::Display for FrameError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            FrameError::TooLarge { len } => write!(f, "frame too large: {}", len),
            FrameError::Protocol(e) => write!(f, "protocol error: {}", e),
            FrameError::UnexpectedEof { needed, remaining } => {
                write!(f, "unexpected eof (needed {}, remaining {})", needed, remaining)
            }
        }
    }
}

impl std::error::Error for FrameError {}

pub type Result<T> = std::result::Result<T, FrameError>;

pub const MAX_FRAME_LEN: u32 = 8 * 1024 * 1024; // 8 MiB

/// Encode 1 message thành frame: u32_be_len + payload.
pub fn encode_frame(msg: &Message) -> Result<Vec<u8>> {
    let payload = encode_message(msg).map_err(FrameError::Protocol)?;
    let len_u32: u32 = payload.len().try_into().unwrap_or(u32::MAX);
    if len_u32 > MAX_FRAME_LEN {
        return Err(FrameError::TooLarge { len: len_u32 });
    }
    let mut out = Vec::with_capacity(4 + payload.len());
    out.extend_from_slice(&len_u32.to_be_bytes());
    out.extend_from_slice(&payload);
    Ok(out)
}

/// Decode 1 frame từ buffer.
/// Trả: (message, bytes_consumed).
pub fn decode_frame(buf: &[u8]) -> Result<(Message, usize)> {
    if buf.len() < 4 {
        return Err(FrameError::UnexpectedEof {
            needed: 4,
            remaining: buf.len(),
        });
    }
    let len = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]);
    if len > MAX_FRAME_LEN {
        return Err(FrameError::TooLarge { len });
    }
    let needed = 4usize + len as usize;
    if buf.len() < needed {
        return Err(FrameError::UnexpectedEof {
            needed,
            remaining: buf.len(),
        });
    }
    let payload = &buf[4..needed];
    let msg = decode_message(payload).map_err(FrameError::Protocol)?;
    Ok((msg, needed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Tip, Message};
    use egg_types::Hash256;

    #[test]
    fn frame_roundtrip() {
        let m = Message::Ping { nonce: 7 };
        let f = encode_frame(&m).unwrap();
        let (back, used) = decode_frame(&f).unwrap();
        assert_eq!(used, f.len());
        assert_eq!(m, back);
    }

    #[test]
    fn frame_multiple_in_buffer() {
        let a = Message::Pong { nonce: 1 };
        let b = Message::Hello {
            chain_id: 1,
            genesis_id: Hash256([1u8; 32]),
            tip: Tip { height: 0, hash: Hash256([2u8; 32]) },
            node_nonce: 9,
            agent: "x".to_string(),
        };

        let fa = encode_frame(&a).unwrap();
        let fb = encode_frame(&b).unwrap();

        let mut buf = Vec::new();
        buf.extend_from_slice(&fa);
        buf.extend_from_slice(&fb);

        let (ma, ua) = decode_frame(&buf).unwrap();
        assert_eq!(ma, a);

        let (mb, ub) = decode_frame(&buf[ua..]).unwrap();
        assert_eq!(mb, b);
        assert_eq!(ua + ub, buf.len());
    }
}
