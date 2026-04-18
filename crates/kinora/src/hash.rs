use std::fmt;
use std::str::FromStr;

const HASH_HEX_LEN: usize = 64;
pub const SHORTHASH_LEN: usize = 8;

#[derive(Debug, Clone, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct Hash(String);

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum HashParseError {
    WrongLength { got: usize },
    NotLowerHex,
}

impl fmt::Display for HashParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HashParseError::WrongLength { got } => {
                write!(f, "hash must be {HASH_HEX_LEN} hex chars, got {got}")
            }
            HashParseError::NotLowerHex => {
                write!(f, "hash must be lowercase hex [0-9a-f]")
            }
        }
    }
}

impl std::error::Error for HashParseError {}

impl Hash {
    pub fn of_content(content: &[u8]) -> Self {
        let hex = blake3::hash(content).to_hex().to_string();
        Self(hex)
    }

    pub fn as_hex(&self) -> &str {
        &self.0
    }

    pub fn shorthash(&self) -> &str {
        &self.0[..SHORTHASH_LEN]
    }

    pub fn shard(&self) -> &str {
        &self.0[..2]
    }
}

impl FromStr for Hash {
    type Err = HashParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len() != HASH_HEX_LEN {
            return Err(HashParseError::WrongLength { got: s.len() });
        }
        if !s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
            return Err(HashParseError::NotLowerHex);
        }
        Ok(Self(s.to_owned()))
    }
}

impl fmt::Display for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_of_empty_is_blake3_empty() {
        let h = Hash::of_content(b"");
        assert_eq!(
            h.as_hex(),
            "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
        );
    }

    #[test]
    fn hash_output_is_64_hex_chars() {
        let h = Hash::of_content(b"hello");
        assert_eq!(h.as_hex().len(), 64);
        assert!(h.as_hex().chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    }

    #[test]
    fn same_content_same_hash() {
        let a = Hash::of_content(b"kinora");
        let b = Hash::of_content(b"kinora");
        assert_eq!(a, b);
    }

    #[test]
    fn different_content_different_hash() {
        let a = Hash::of_content(b"kinora");
        let b = Hash::of_content(b"kinoral");
        assert_ne!(a, b);
    }

    #[test]
    fn parse_valid_hex() {
        let h: Hash = "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
            .parse()
            .unwrap();
        assert_eq!(h.as_hex().len(), 64);
    }

    #[test]
    fn parse_rejects_short() {
        let err = "abc".parse::<Hash>().unwrap_err();
        assert!(matches!(err, HashParseError::WrongLength { got: 3 }));
    }

    #[test]
    fn parse_rejects_long() {
        let s = "a".repeat(65);
        let err = s.parse::<Hash>().unwrap_err();
        assert!(matches!(err, HashParseError::WrongLength { got: 65 }));
    }

    #[test]
    fn parse_rejects_uppercase() {
        let s = "AF1349B9F5F9A1A6A0404DEA36DCC9499BCB25C9ADC112B7CC9A93CAE41F3262";
        let err = s.parse::<Hash>().unwrap_err();
        assert_eq!(err, HashParseError::NotLowerHex);
    }

    #[test]
    fn parse_rejects_non_hex() {
        let s = "z".repeat(64);
        let err = s.parse::<Hash>().unwrap_err();
        assert_eq!(err, HashParseError::NotLowerHex);
    }

    #[test]
    fn shorthash_is_first_8() {
        let h = Hash::of_content(b"");
        assert_eq!(h.shorthash(), "af1349b9");
    }

    #[test]
    fn shard_is_first_2() {
        let h = Hash::of_content(b"");
        assert_eq!(h.shard(), "af");
    }

    #[test]
    fn roundtrip_parse_display() {
        let a = Hash::of_content(b"roundtrip");
        let s = a.to_string();
        let b: Hash = s.parse().unwrap();
        assert_eq!(a, b);
    }
}
