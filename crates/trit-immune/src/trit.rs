//! 三值逻辑核心：`Trit ∈ {-1, 0, +1}`。
//!
//! # 安全语义映射（本 crate 的领域约定）
//!
//! | 值 | 极性 | 安全含义 |
//! |----|------|----------|
//! | `-1` | 负 | **漏洞确认**：渗透测试 Agent 攻击成功（SQL 注入、鉴权绕过、Prompt 注入……） |
//! | ` 0` | 零 | **未知**：发现可疑攻击面但证据不足；分支冻结、沙盒挂起、留给人裁量 |
//! | `+1` | 正 | **防御成功**：攻击被拦截，或不存在可利用面 |
//!
//! 逻辑上采用 **Kleene K3**：`0` 是"信息缺失"，只在能被确定时才被确定——
//! `-1 & x == -1`（已证伪即否决），`+1 | x == +1`（已证安全即放行）。
//! 另提供 Łukasiewicz Ł3 蕴含作对照（Ł3 的 `0 => 0 == +1`，K3 保守地给 `0`）。

use std::fmt;
use std::ops::{BitAnd, BitOr, BitXor, Not};
use std::str::FromStr;

/// 单个三进制位（trit）：漏洞确认 / 未知 / 防御成功。
/// 派生序 = 声明序：`Vulnerable < Unknown < Hardened`（坏→好），
/// 供"变坏才落账"等单调性判断使用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(i8)]
pub enum Trit {
    /// `-1`：攻击成功，漏洞确证（K3 的 False）。
    Vulnerable = -1,
    /// `0`：未知面 / 沙盒挂起 / 待人裁（K3 的 Unknown）。
    Unknown = 0,
    /// `+1`：防御成功，无可利用面（K3 的 True）。
    Hardened = 1,
}

impl Trit {
    /// 数值视图（平衡三进制值）。
    pub fn to_i8(self) -> i8 {
        self as i8
    }

    /// 从平衡三进制值构造；越界返回 `None`。
    pub fn from_i8(v: i8) -> Option<Self> {
        match v {
            -1 => Some(Self::Vulnerable),
            0 => Some(Self::Unknown),
            1 => Some(Self::Hardened),
            _ => None,
        }
    }

    /// 紧凑记法：`-` / `0` / `+`。
    pub fn from_char(c: char) -> Option<Self> {
        match c {
            '-' => Some(Self::Vulnerable),
            '0' => Some(Self::Unknown),
            '+' => Some(Self::Hardened),
            _ => None,
        }
    }

    pub fn as_char(self) -> char {
        match self {
            Self::Vulnerable => '-',
            Self::Unknown => '0',
            Self::Hardened => '+',
        }
    }

    /// 是否已有确定裁决（非 0）。
    pub fn is_known(self) -> bool {
        self != Self::Unknown
    }

    /// 展示符号：`✗` / `?` / `✓`。
    pub fn symbol(self) -> &'static str {
        match self {
            Self::Vulnerable => "✗",
            Self::Unknown => "?",
            Self::Hardened => "✓",
        }
    }

    // ── Kleene K3 连接词 ────────────────────────────────────────

    /// K3 非：`0` 的否定仍是 `0`。
    pub fn not(self) -> Trit {
        match self {
            Self::Vulnerable => Self::Hardened,
            Self::Unknown => Self::Unknown,
            Self::Hardened => Self::Vulnerable,
        }
    }

    /// Kleene AND：`-1` 支配（已证伪即否决），否则 `0` 传染。
    pub fn and(self, rhs: Trit) -> Trit {
        use Trit::*;
        match (self, rhs) {
            (Vulnerable, _) | (_, Vulnerable) => Vulnerable,
            (Unknown, _) | (_, Unknown) => Unknown,
            (Hardened, Hardened) => Hardened,
        }
    }

    /// Kleene OR：`+1` 支配（已证安全即放行），否则 `0` 传染。
    pub fn or(self, rhs: Trit) -> Trit {
        use Trit::*;
        match (self, rhs) {
            (Hardened, _) | (_, Hardened) => Hardened,
            (Unknown, _) | (_, Unknown) => Unknown,
            (Vulnerable, Vulnerable) => Vulnerable,
        }
    }

    /// 经典 XOR：操作数全部确定时按二值异或，任一未知则未知。
    pub fn xor(self, rhs: Trit) -> Trit {
        use Trit::*;
        match (self, rhs) {
            (Unknown, _) | (_, Unknown) => Unknown,
            (a, b) if a != b => Hardened,
            _ => Vulnerable,
        }
    }

    /// K3 蕴含 `¬a ∨ b`。策略语义示例：`可疑 => 冻结`——
    /// 前提未知时结论也只能是未知（不武断放行）。
    pub fn implies_k3(self, rhs: Trit) -> Trit {
        self.not().or(rhs)
    }

    /// Ł3 蕴含 `min(+1, b - a + 1)`：与 K3 的差别在 `0 => 0 == +1`
    /// （Ł3 认为"未知前提蕴含未知结论"仍可确立，K3 保守地给 `0`）。
    pub fn implies_l3(self, rhs: Trit) -> Trit {
        let v = rhs.to_i8() - self.to_i8() + 1;
        Trit::from_i8(v.min(1)).unwrap_or(Self::Hardened)
    }

    /// K3 AND 折叠。空集为 `+1`（无可指摘）——门禁应自行确认覆盖面非空。
    pub fn fold_and<I: IntoIterator<Item = Trit>>(iter: I) -> Trit {
        iter.into_iter().fold(Self::Hardened, Trit::and)
    }

    /// K3 OR 折叠。空集为 `-1`。
    pub fn fold_or<I: IntoIterator<Item = Trit>>(iter: I) -> Trit {
        iter.into_iter().fold(Self::Vulnerable, Trit::or)
    }
}

impl Not for Trit {
    type Output = Trit;
    fn not(self) -> Trit {
        Trit::not(self)
    }
}

impl BitAnd for Trit {
    type Output = Trit;
    fn bitand(self, rhs: Trit) -> Trit {
        self.and(rhs)
    }
}

impl BitOr for Trit {
    type Output = Trit;
    fn bitor(self, rhs: Trit) -> Trit {
        self.or(rhs)
    }
}

impl BitXor for Trit {
    type Output = Trit;
    fn bitxor(self, rhs: Trit) -> Trit {
        self.xor(rhs)
    }
}

impl fmt::Display for Trit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Vulnerable => write!(f, "-1"),
            Self::Unknown => write!(f, "0"),
            Self::Hardened => write!(f, "+1"),
        }
    }
}

impl FromStr for Trit {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "-1" | "-" | "v" | "vuln" | "vulnerable" | "exploited" => Ok(Self::Vulnerable),
            "0" | "?" | "u" | "unknown" | "suspect" => Ok(Self::Unknown),
            "+1" | "1" | "+" | "h" | "safe" | "hardened" => Ok(Self::Hardened),
            other => Err(format!("无法解析为 trit：{other}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operator_sugar() {
        assert_eq!(!Trit::Unknown, Trit::Unknown);
        assert_eq!(Trit::Vulnerable & Trit::Hardened, Trit::Vulnerable);
        assert_eq!(Trit::Unknown | Trit::Hardened, Trit::Hardened);
        assert_eq!(Trit::Hardened ^ Trit::Hardened, Trit::Vulnerable);
    }

    #[test]
    fn roundtrip() {
        for t in [Trit::Vulnerable, Trit::Unknown, Trit::Hardened] {
            assert_eq!(Trit::from_i8(t.to_i8()), Some(t));
            assert_eq!(Trit::from_char(t.as_char()), Some(t));
            assert_eq!(t.to_string().parse::<Trit>(), Ok(t));
        }
    }
}
