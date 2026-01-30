#![forbid(unsafe_code)]

use egg_types::{Hash256, Height};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpcRequest {
    pub id: u64,
    pub method: String,
    pub params: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpcResponse {
    pub id: u64,
    pub ok: bool,
    pub result: Option<serde_json::Value>,
    pub error: Option<RpcError>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainTip {
    pub height: Height,
    pub hash: Hash256,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_roundtrip_request() {
        let req = RpcRequest {
            id: 1,
            method: "chain_tip".to_string(),
            params: serde_json::json!({}),
        };
        let s = serde_json::to_string(&req).unwrap();
        let back: RpcRequest = serde_json::from_str(&s).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn json_roundtrip_response_ok() {
        let resp = RpcResponse {
            id: 1,
            ok: true,
            result: Some(serde_json::json!({"height": 0})),
            error: None,
        };
        let s = serde_json::to_string(&resp).unwrap();
        let back: RpcResponse = serde_json::from_str(&s).unwrap();
        assert_eq!(resp, back);
    }
}
