use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestPage {
    pub page_id: String,
    pub head_seq: u64,
    pub frontier: String,
    pub snapshot_seq: u64,
    pub tail_bytes: u64,
    pub updated_at: i64,
    pub deleted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub rev: u64,
    pub protocol_version: u32,
    pub pages: Vec<ManifestPage>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub instance: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateIn {
    pub id: String,
    pub client_seq: i64,
    pub frontier: String,
    #[serde(with = "b64")]
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushRequest {
    pub updates: Vec<UpdateIn>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushResponse {
    pub accepted: usize,
    pub head_seq: u64,
    pub duplicate: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateOut {
    pub seq: u64,
    pub id: String,
    pub frontier: String,
    #[serde(with = "b64")]
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullResponse {
    #[serde(default, skip_serializing_if = "Option::is_none", with = "b64_opt")]
    pub snapshot: Option<Vec<u8>>,
    pub updates: Vec<UpdateOut>,
    pub head_seq: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotRequest {
    pub covers_seq: u64,
    #[serde(with = "b64")]
    pub payload: Vec<u8>,
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = match chunk.len() {
            3 => (chunk[0] as u32) << 16 | (chunk[1] as u32) << 8 | chunk[2] as u32,
            2 => (chunk[0] as u32) << 16 | (chunk[1] as u32) << 8,
            _ => (chunk[0] as u32) << 16,
        };
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

pub fn decode(s: &str) -> Option<Vec<u8>> {
    let s = s.trim_end_matches('=');
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut acc = 0u32;
    let mut bits = 0;
    for c in s.bytes() {
        let v = ALPHABET.iter().position(|a| *a == c)? as u32;
        acc = acc << 6 | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits & 0xff) as u8);
        }
    }
    Some(out)
}

mod b64 {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&super::encode(v))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        super::decode(&s).ok_or_else(|| serde::de::Error::custom("invalid base64"))
    }
}

mod b64_opt {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &Option<Vec<u8>>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(b) => s.serialize_str(&super::encode(b)),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<u8>>, D::Error> {
        match Option::<String>::deserialize(d)? {
            Some(s) => super::decode(&s)
                .map(Some)
                .ok_or_else(|| serde::de::Error::custom("invalid base64")),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips() {
        for n in 0..20 {
            let bytes: Vec<u8> = (0..n).map(|i| (i * 37 + 11) as u8).collect();
            assert_eq!(decode(&encode(&bytes)).unwrap(), bytes);
        }
        assert_eq!(encode(b"Man"), "TWFu");
        assert_eq!(encode(b"Ma"), "TWE=");
        assert!(decode("!!").is_none());
    }

    #[test]
    fn push_request_matches_protocol_shape() {
        let req = PushRequest {
            updates: vec![UpdateIn {
                id: "c1:41".into(),
                client_seq: 41,
                frontier: "ab".into(),
                payload: vec![1, 2, 3],
            }],
        };
        let v = serde_json::to_value(&req).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"updates": [{"id": "c1:41", "client_seq": 41, "frontier": "ab", "payload": "AQID"}]})
        );
        let back: PushRequest = serde_json::from_value(v).unwrap();
        assert_eq!(back, req);
        let pull = PullResponse {
            snapshot: None,
            updates: vec![],
            head_seq: 3,
        };
        let v = serde_json::to_value(&pull).unwrap();
        assert!(v.get("snapshot").is_none());
        assert_eq!(serde_json::from_value::<PullResponse>(v).unwrap(), pull);
    }
}
