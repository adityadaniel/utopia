use std::fmt;

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const NS_PAGE: Uuid = Uuid::from_u128(0x7f3c_1a52_9d4e_4b0a_8c61_2e5f_0b9d_a101);
pub const NS_DAILY: Uuid = Uuid::from_u128(0x7f3c_1a52_9d4e_4b0a_8c61_2e5f_0b9d_a102);

#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PageId(pub String);

#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BlockId(pub String);

impl PageId {
    pub fn for_title_key(title_key: &str) -> Self {
        PageId(Uuid::new_v5(&NS_PAGE, format!("title:{title_key}").as_bytes()).to_string())
    }

    pub fn for_date(date: NaiveDate) -> Self {
        PageId(
            Uuid::new_v5(
                &NS_DAILY,
                format!("daily:{}", date.format("%Y-%m-%d")).as_bytes(),
            )
            .to_string(),
        )
    }

    pub fn fresh() -> Self {
        PageId(ulid::Ulid::new().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl BlockId {
    pub fn new() -> Self {
        BlockId(ulid::Ulid::new().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn timestamp_ms(&self) -> Option<i64> {
        ulid::Ulid::from_string(&self.0)
            .ok()
            .map(|u| u.timestamp_ms() as i64)
    }

    pub fn is_valid_ref(s: &str) -> bool {
        (1..=64).contains(&s.len())
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    }
}

impl Default for BlockId {
    fn default() -> Self {
        Self::new()
    }
}

macro_rules! id_fmt {
    ($t:ty) => {
        impl fmt::Display for $t {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
        impl fmt::Debug for $t {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($t), self.0)
            }
        }
        impl From<&str> for $t {
            fn from(s: &str) -> Self {
                Self(s.to_string())
            }
        }
        impl From<String> for $t {
            fn from(s: String) -> Self {
                Self(s)
            }
        }
    };
}
id_fmt!(PageId);
id_fmt!(BlockId);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_page_ids_converge() {
        assert_eq!(
            PageId::for_title_key("alpha"),
            PageId::for_title_key("alpha")
        );
        assert_ne!(
            PageId::for_title_key("alpha"),
            PageId::for_title_key("beta")
        );
        let d = NaiveDate::from_ymd_opt(2026, 11, 2).unwrap();
        assert_eq!(PageId::for_date(d), PageId::for_date(d));
        assert_ne!(PageId::for_date(d), PageId::for_title_key("2026-11-02"));
    }

    #[test]
    fn block_ids_are_ulids() {
        let id = BlockId::new();
        assert_eq!(id.as_str().len(), 26);
        assert!(id.timestamp_ms().is_some());
        assert!(BlockId::is_valid_ref(id.as_str()));
        assert!(!BlockId::is_valid_ref("has space"));
        assert!(!BlockId::is_valid_ref(""));
    }
}
