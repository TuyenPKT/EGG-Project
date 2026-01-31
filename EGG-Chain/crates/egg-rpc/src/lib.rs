#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug)]
pub enum RpcCodecError {
    Json(serde_json::Error),
}

impl core::fmt::Display for RpcCodecError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            RpcCodecError::Json(e) => write!(f, "json: {}", e),
        }
    }
}

impl std::error::Error for RpcCodecError {}

impl From<serde_json::Error> for RpcCodecError {
    fn from(value: serde_json::Error) -> Self {
        RpcCodecError::Json(value)
    }
}

pub type Result<T> = core::result::Result<T, RpcCodecError>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RpcMethod {
    PeerHealth,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpcRequest {
    pub id: u64,
    pub method: RpcMethod,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerHealth {
    pub penalty_score: i32,
    pub distinct_notfound_ids: usize,
    pub inflight_blocks: usize,
    pub banned: bool,
    pub ban_reason: Option<String>,
}

impl PeerHealth {
    pub fn new(
        penalty_score: i32,
        distinct_notfound_ids: usize,
        inflight_blocks: usize,
        banned: bool,
        ban_reason: Option<String>,
    ) -> Self {
        Self {
            penalty_score,
            distinct_notfound_ids,
            inflight_blocks,
            banned,
            ban_reason,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "method", content = "data", rename_all = "snake_case")]
pub enum RpcResult {
    PeerHealth(PeerHealth),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RpcResponse {
    Ok { id: u64, result: RpcResult },
    Err { id: u64, error: RpcError },
}

pub fn encode_request(req: &RpcRequest) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec(req)?)
}

pub fn decode_request(bytes: &[u8]) -> Result<RpcRequest> {
    Ok(serde_json::from_slice(bytes)?)
}

pub fn encode_response(resp: &RpcResponse) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec(resp)?)
}

pub fn decode_response(bytes: &[u8]) -> Result<RpcResponse> {
    Ok(serde_json::from_slice(bytes)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_roundtrip_json() {
        let req = RpcRequest {
            id: 7,
            method: RpcMethod::PeerHealth,
        };

        let bytes = encode_request(&req).unwrap();
        let got = decode_request(&bytes).unwrap();
        assert_eq!(got, req);
    }

    #[test]
    fn response_ok_peer_health_roundtrip_json() {
        let health = PeerHealth::new(12, 3, 9, false, None);
        let resp = RpcResponse::Ok {
            id: 7,
            result: RpcResult::PeerHealth(health),
        };

        let bytes = encode_response(&resp).unwrap();
        let got = decode_response(&bytes).unwrap();
        assert_eq!(got, resp);
    }

    #[test]
    fn response_err_roundtrip_json() {
        let resp = RpcResponse::Err {
            id: 7,
            error: RpcError {
                code: 4001,
                message: "bad request".to_string(),
            },
        };

        let bytes = encode_response(&resp).unwrap();
        let got = decode_response(&bytes).unwrap();
        assert_eq!(got, resp);
    }
}
