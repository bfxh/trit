//! Kani 模型检查 harness（GOAL.md 升级①完全体）——`#[cfg(kani)]` 门控，
//! 常规编译/测试完全跳过本模块，零依赖不受影响。
//!
//! 性质测试（`tests/properties.rs`）用随机采样，本模块用**符号执行穷举
//! 全部输入空间**：Trit 只有 3 个值，任意二元/三元组合都能被 Kani 完备
//! 覆盖——这是"例子测试说这个输入对、性质测试说采样都对"之后的第三层：
//! **穷举证明**。
//!
//! 运行方式（本机装 Kani 后）：
//!
//! ```text
//! cargo install --locked kani-verifier
//! cargo kani setup
//! cargo kani            # 跑本文件全部 harness
//! ```
//!
//! 覆盖的不变量（与性质测试同源，见 `docs/RESEARCH.md` §四）：
//! 1. K3 AND 交换律 + 结合律（3×3、3×3×3 全空间）；
//! 2. K3 AND 单调性：结果 ≤ 任一操作数（Bad 支配的代数表达）；
//! 3. 门禁单调性：任意裁决列表追加 `-1`，恶劣度不降——merge_gate 的
//!    折叠核心 `Trit::fold_and` 的符号级证明。

#![cfg(kani)]
#![allow(dead_code)]

use crate::trit::Trit;
use crate::verdict::GateDecision;

#[cfg(kani)]
impl kani::Arbitrary for Trit {
    fn any() -> Self {
        let v: u8 = kani::any();
        kani::assume(v < 3);
        match v {
            0 => Trit::Vulnerable,
            1 => Trit::Unknown,
            _ => Trit::Hardened,
        }
    }
}

/// 门禁恶劣度（Allow=0 < Freeze=1 < Block=2）。
fn gate_rank(g: GateDecision) -> u8 {
    match g {
        GateDecision::Allow => 0,
        GateDecision::Freeze => 1,
        GateDecision::Block => 2,
    }
}

#[cfg(kani)]
mod proofs {
    use super::*;
    use crate::verdict::merge_gate;

    #[kani::proof]
    fn k3_and_commutes() {
        let a: Trit = kani::any();
        let b: Trit = kani::any();
        assert_eq!(a.and(b), b.and(a), "K3 AND 交换律（全 3×3 空间）");
    }

    #[kani::proof]
    fn k3_and_associates() {
        let a: Trit = kani::any();
        let b: Trit = kani::any();
        let c: Trit = kani::any();
        assert_eq!(a.and(b).and(c), a.and(b.and(c)), "K3 AND 结合律（全 3³ 空间）");
    }

    #[kani::proof]
    fn k3_and_monotone_below_both_operands() {
        let a: Trit = kani::any();
        let b: Trit = kani::any();
        let r = a.and(b);
        assert!(r <= a && r <= b, "AND 结果不高于任一操作数（Bad 支配）");
    }

    #[kani::proof]
    fn k3_and_idempotent() {
        let a: Trit = kani::any();
        assert_eq!(a.and(a), a, "幂等：a∧a = a");
    }

    #[kani::proof]
    fn merge_gate_monotone_under_extra_negative() {
        // 任意三裁决列表 + 追加一个 -1：恶劣度不降（门禁单调的折叠核）
        let list: [Trit; 3] = kani::any();
        let mut longer = list.to_vec();
        longer.push(Trit::Vulnerable);
        let before = gate_rank(merge_gate(list));
        let after = gate_rank(merge_gate(longer));
        assert!(after >= before, "追加 -1 后门禁不得升级");
    }

    #[kani::proof]
    fn merge_gate_full_plus_one_is_allow() {
        let list: [Trit; 4] = kani::any();
        kani::assume(list.iter().all(|&t| t == Trit::Hardened));
        assert_eq!(merge_gate(list), GateDecision::Allow, "全 +1 必放行");
    }
}
