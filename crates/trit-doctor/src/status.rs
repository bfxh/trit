//! 四值状态格：doctor 的 OK / WARN / FAIL / UNAVAILABLE 与 Belnap FOUR 同构。
//!
//! | doctor | Belnap | 双轨 (真, 假) | 含义 |
//! |---|---|---|---|
//! | OK | T | (1, 0) | 证据在场且为真 |
//! | WARN | B | (1, 1) | 同时有绿证据和红证据（债务/未固化） |
//! | FAIL | F | (0, 1) | 证据在场且为假 |
//! | UNAVAILABLE | N | (0, 0) | 证据缺席，不能判真假 |
//!
//! `B4::and` 是这里的聚合算子：任一 FAIL 压过 WARN，任一 UNAVAILABLE 在没有 FAIL 时
//! 压过 OK/WARN。这不是“平均分”，而是“最弱证据决定整体”。

use serde::Serialize;
use trit_belnap::B4;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Status {
    Ok,
    Warn,
    Fail,
    Unavailable,
}

impl Status {
    #[must_use]
    pub const fn as_b4(self) -> B4 {
        match self {
            Self::Ok => B4::T,
            Self::Warn => B4::B,
            Self::Fail => B4::F,
            Self::Unavailable => B4::N,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "OK",
            Self::Warn => "WARN",
            Self::Fail => "FAIL",
            Self::Unavailable => "UNAVAILABLE",
        }
    }

    #[must_use]
    pub fn from_code(value: &str) -> Option<Self> {
        match value {
            "OK" => Some(Self::Ok),
            "WARN" => Some(Self::Warn),
            "FAIL" => Some(Self::Fail),
            "UNAVAILABLE" => Some(Self::Unavailable),
            _ => None,
        }
    }
}

impl From<B4> for Status {
    fn from(value: B4) -> Self {
        match value {
            B4::T => Self::Ok,
            B4::B => Self::Warn,
            B4::F => Self::Fail,
            B4::N => Self::Unavailable,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_values_round_trip() {
        for status in [Status::Ok, Status::Warn, Status::Fail, Status::Unavailable] {
            assert_eq!(Status::from(status.as_b4()), status);
            assert_eq!(Status::from_code(status.as_str()), Some(status));
        }
    }

    #[test]
    fn aggregate_takes_the_weakest_evidence() {
        let all = [Status::Ok, Status::Warn, Status::Fail, Status::Unavailable];
        let fold = |left: Status, right: Status| Status::from(left.as_b4().and(right.as_b4()));
        assert_eq!(fold(Status::Ok, Status::Ok), Status::Ok);
        assert_eq!(fold(Status::Ok, Status::Warn), Status::Warn);
        assert_eq!(fold(Status::Warn, Status::Fail), Status::Fail);
        assert_eq!(fold(Status::Ok, Status::Unavailable), Status::Unavailable);
        assert_eq!(fold(Status::Unavailable, Status::Fail), Status::Fail);
        for status in all {
            assert_eq!(fold(Status::Ok, status), status);
        }
        // 单看四个值，聚合确实能回到最弱的那一个（FAIL 与 UNAVAILABLE 同时出现时取 FAIL）。
        assert_eq!(fold(Status::Fail, Status::Unavailable), Status::Fail);
    }
}
