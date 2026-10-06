//! 编号与时间。全是值类型；时间是调用方传进来的 Unix 毫秒，本 crate 从不读时钟。
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

pub const MINUTE: i64 = 60_000;
pub const HOUR: i64 = 60 * MINUTE;

/// Unix 毫秒。只比较、只加减，不做日历运算。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Ts(pub i64);

impl Ts {
    pub fn plus(self, ms: i64) -> Ts {
        Ts(self.0.saturating_add(ms))
    }

    /// `self - earlier`，毫秒；`earlier` 在后面时为负。
    pub fn since(self, earlier: Ts) -> i64 {
        self.0.saturating_sub(earlier.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseIdError {
    pub input: String,
    pub expected: &'static str,
}

impl fmt::Display for ParseIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "`{}` is not a valid {}", self.input, self.expected)
    }
}

impl std::error::Error for ParseIdError {}

/// `<前缀><正整数>`，前缀不分大小写，数字不带符号、不带前导 0（规范写法唯一）。
fn parse_prefixed(s: &str, prefix: char, expected: &'static str) -> Result<u64, ParseIdError> {
    let err = || ParseIdError {
        input: s.to_string(),
        expected,
    };
    let mut chars = s.chars();
    let first = chars.next().ok_or_else(err)?;
    if !first.eq_ignore_ascii_case(&prefix) {
        return Err(err());
    }
    let digits = chars.as_str();
    if digits.is_empty() || digits.starts_with('0') || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(err());
    }
    digits.parse::<u64>().map_err(|_| err())
}

macro_rules! numbered_id {
    ($name:ident, $int:ty, $prefix:literal, $expected:literal) => {
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(pub $int);

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}{}", $prefix, self.0)
            }
        }

        impl FromStr for $name {
            type Err = ParseIdError;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                let n = parse_prefixed(s, $prefix, $expected)?;
                <$int>::try_from(n).map($name).map_err(|_| ParseIdError {
                    input: s.to_string(),
                    expected: $expected,
                })
            }
        }

        impl TryFrom<String> for $name {
            type Error = ParseIdError;
            fn try_from(s: String) -> Result<Self, Self::Error> {
                s.parse()
            }
        }

        impl From<$name> for String {
            fn from(id: $name) -> String {
                id.to_string()
            }
        }
    };
}

numbered_id!(WorkId, u32, 'W', "work item id (W<n>)");
numbered_id!(AskId, u32, 'H', "human ask id (H<n>)");
numbered_id!(EntryNo, i64, '#', "entry todo number (#<n>)");

macro_rules! opaque_id {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            pub fn new(s: impl Into<String>) -> Self {
                $name(s.into())
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

opaque_id!(SessionId);
opaque_id!(CrewId);
opaque_id!(ChannelId);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_print_with_their_prefix() {
        assert_eq!(WorkId(3).to_string(), "W3");
        assert_eq!(AskId(12).to_string(), "H12");
        assert_eq!(EntryNo(7).to_string(), "#7");
    }

    #[test]
    fn ids_parse_canonical_and_lowercase_prefix() {
        assert_eq!("W3".parse::<WorkId>(), Ok(WorkId(3)));
        assert_eq!("w3".parse::<WorkId>(), Ok(WorkId(3)));
        assert_eq!("h1".parse::<AskId>(), Ok(AskId(1)));
        assert_eq!("#42".parse::<EntryNo>(), Ok(EntryNo(42)));
    }

    #[test]
    fn wrong_prefix_or_number_shape_is_rejected() {
        for bad in [
            "", "W", "3", "#3", "H3", "W03", "W0", "W-1", "W+1", "W 3", "W3 ", "W3a",
        ] {
            assert!(bad.parse::<WorkId>().is_err(), "{bad:?} 应被拒");
        }
        assert!(
            "W3".parse::<EntryNo>().is_err(),
            "W 号不能当 #N 用（两本账分开）"
        );
        assert!("#3".parse::<WorkId>().is_err());
        assert!("W4294967296".parse::<WorkId>().is_err(), "超出 u32");
        let err = "X1".parse::<WorkId>().unwrap_err();
        assert_eq!(err.to_string(), "`X1` is not a valid work item id (W<n>)");
    }

    #[test]
    fn ids_serialize_as_their_printed_form() {
        assert_eq!(serde_json::to_string(&WorkId(5)).unwrap(), "\"W5\"");
        assert_eq!(serde_json::from_str::<WorkId>("\"W5\"").unwrap(), WorkId(5));
        assert!(serde_json::from_str::<WorkId>("\"#5\"").is_err());
        assert!(serde_json::from_str::<WorkId>("5").is_err());
        assert_eq!(serde_json::to_string(&EntryNo(9)).unwrap(), "\"#9\"");
        assert_eq!(
            serde_json::to_string(&SessionId::new("s-1")).unwrap(),
            "\"s-1\""
        );
        assert_eq!(serde_json::to_string(&Ts(1_000)).unwrap(), "1000");
    }

    #[test]
    fn ids_order_numerically_not_lexically() {
        let mut v = vec![WorkId(10), WorkId(2), WorkId(1)];
        v.sort();
        assert_eq!(v, [WorkId(1), WorkId(2), WorkId(10)]);
    }

    #[test]
    fn ts_arithmetic_saturates() {
        assert_eq!(Ts(10).plus(5), Ts(15));
        assert_eq!(Ts(i64::MAX).plus(1), Ts(i64::MAX));
        assert_eq!(Ts(10).since(Ts(4)), 6);
        assert_eq!(Ts(4).since(Ts(10)), -6);
        assert_eq!(HOUR, 3_600_000);
    }
}
