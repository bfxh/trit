//! 三值点积的 popcount 形式：4 → 3 → 2 的等价关系，以及它们各自的前提。
//!
//! 双导轨约定：`pos` 位为 1 表示 +1，`neg` 位为 1 表示 -1，两位都 0 表示 0。
//!
//! 三种写法：
//!
//! ```text
//! 4-popc: popc(ap&bp) + popc(an&bn) - popc(ap&bn) - popc(an&bp)
//! 3-popc: popc(ap&bp) + popc(an&bn) - popc((ap&bn) | (an&bp))     // 合并交叉项
//! 2-popc: popc((ap&bp) | (an&bn)) - popc((ap&bn) | (an&bp))       // 同号项也合并
//! ```
//!
//! # 实测结论（穷举，不是推断）
//!
//! - **2-popc 与 4-popc 恒等**：4 lane 的全部 65,536 种组合（含 rails 交叠的非法输入）
//!   逐一比较，**0 处不等**。
//! - **3-popc 反而需要 `ap&an == 0 && bp&bn == 0`**：同样穷举下，交叠输入有 14,911 处不等。
//!
//! 原因：`popc(x) + popc(y) = popc(x|y)` 要求 `x & y == 0`。
//! 交叉项 `(ap&bn) & (an&bp) = ap&an&bn&bp`，rails 不交叠时为 0；
//! 同号项 `(ap&bp) & (an&bn)` 也是 `ap&an&bp&bn`。两者是**同一个集合**，
//! 所以 2-popc 里 `same` 多算的位恰好被 `opp` 多算的位抵消——不变量自己消失了。
//!
//! 结论：2-popc 不只是省一条 popcount，它**比现有 3-popc 更鲁棒**。

fn dot_four_popc(ap: u32, an: u32, bp: u32, bn: u32) -> i32 {
    (ap & bp).count_ones() as i32 + (an & bn).count_ones() as i32
        - (ap & bn).count_ones() as i32
        - (an & bp).count_ones() as i32
}

fn dot_three_popc(ap: u32, an: u32, bp: u32, bn: u32) -> i32 {
    (ap & bp).count_ones() as i32 + (an & bn).count_ones() as i32
        - ((ap & bn) | (an & bp)).count_ones() as i32
}

fn dot_two_popc(ap: u32, an: u32, bp: u32, bn: u32) -> i32 {
    let same = (ap & bp) | (an & bn);
    let opposite = (ap & bn) | (an & bp);
    same.count_ones() as i32 - opposite.count_ones() as i32
}

/// 穷举 `bits` 位 lane 空间里**全部** 4^bits 组合（不假设 rails 不交叠）。
fn exhaustive_all(bits: u32) {
    let span = 1u32 << bits;
    let mut mismatch_two = 0u32;
    let mut mismatch_three = 0u32;
    for ap in 0..span {
        for an in 0..span {
            for bp in 0..span {
                for bn in 0..span {
                    let want = dot_four_popc(ap, an, bp, bn);
                    if dot_two_popc(ap, an, bp, bn) != want {
                        mismatch_two += 1;
                    }
                    if dot_three_popc(ap, an, bp, bn) != want {
                        mismatch_three += 1;
                    }
                }
            }
        }
    }
    assert_eq!(
        mismatch_two, 0,
        "2-popc 必须与 4-popc 恒等（{bits} bit 空间）"
    );
    // 3-popc 只在 rails 不交叠时等价；交叠时必然出现不等，这里只断言「存在」
    if bits >= 2 {
        assert!(
            mismatch_three > 0,
            "3-popc 在交叠输入上应当不等，否则说明前提被削弱了"
        );
    }
}

#[test]
fn two_popc_is_identical_on_all_two_bit_inputs() {
    exhaustive_all(2); // 4^2 = 256 组合
}

#[test]
fn two_popc_is_identical_on_all_four_bit_inputs() {
    exhaustive_all(4); // 4^4 = 65,536 组合
}

/// 合法 rails（pos & neg == 0）下三种写法全部一致。
#[test]
fn all_three_agree_on_valid_rails_100k() {
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state as u32
    };
    for _ in 0..100_000 {
        let ap = next();
        let an = next() & !ap;
        let bp = next();
        let bn = next() & !bp;
        assert_eq!(ap & an, 0);
        assert_eq!(bp & bn, 0);
        let want = dot_four_popc(ap, an, bp, bn);
        assert_eq!(dot_three_popc(ap, an, bp, bn), want);
        assert_eq!(dot_two_popc(ap, an, bp, bn), want);
    }
}

/// 具体反例：rails 交叠时 3-popc 与 4-popc 不等，而 2-popc 仍然正确。
#[test]
fn overlapping_rails_break_three_popc_but_not_two_popc() {
    // 四个 rail 在同一 lane 全为 1：ap&an&bp&bn != 0
    let ap = 0b01u32;
    let an = 0b01u32;
    let bp = 0b01u32;
    let bn = 0b01u32;

    let four = dot_four_popc(ap, an, bp, bn);
    let three = dot_three_popc(ap, an, bp, bn);
    let two = dot_two_popc(ap, an, bp, bn);

    assert_eq!(two, four, "2-popc 在交叠输入上仍等于 4-popc");
    assert_ne!(three, four, "3-popc 在交叠输入上会算错");
}

/// 恒等式的代数来源：`same` 与 `opp` 的重复计数是同一个集合 `ap&an&bp&bn`。
#[test]
fn duplicate_counts_cancel_exactly() {
    let mut state = 0x1234_5678_9ABC_DEF0u64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state as u32
    };
    for _ in 0..10_000 {
        let ap = next();
        let an = next();
        let bp = next();
        let bn = next();
        let dup_same = (ap & bp) & (an & bn);
        let dup_opp = (ap & bn) & (an & bp);
        assert_eq!(dup_same, dup_opp, "两处重复计数必须同一个集合");
    }
}
