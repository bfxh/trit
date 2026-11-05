//! ARM64 块遍历（N2 方向库 +1 项）：定宽 4 字节步进 + 控制流分类器。
//!
//! 与 x86 侧（[`super::x86len`] + 递归下降）的分工：ARM64 指令**定宽 4 字节**
//! ——没有长度解码问题，但控制流分类器要自己写（B/BL/B.cond/CBZ/CBNZ/
//! TBZ/TBNZ/RET/BR/BLR）。[`arm64_walk`] 从可信种子线性走 + 控制流边扩展，
//! 产出与 x86 侧同型的 [`WalkOutput`]——起点合成、补隙合并、「修上游」
//! 语义全部复用，零分支差异。
//!
//! fail-closed 纪律同 x86 侧：步进错位/越界 = 停链；目标出可执行域 = 不追；
//! 访问数触顶停走。BR（间接跳）/BLR（间接调用）无静态目标——与 x86 侧
//! 间接 call 同语义（不猜寄存器值）。
//!
//! 已知边界：ADRP+ADD 取址对（ARM64 版的 addr_taken）需要跨指令配对状态
//! ——留下一轮；BLR 调用的函数指针来源（vtable）同样留待间接调用数据流。

use super::cfg::{WalkOutput, in_exec_any, section_of};
use std::collections::BTreeSet;

/// ARM64 指令定宽。
pub(super) const ARM64_INSN_LEN: u64 = 4;

/// ARM64 控制流种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ArmKind {
    /// BL：直接调用。
    Call,
    /// B：无条件跳。
    Jump,
    /// 条件跳（B.cond / CBZ / CBNZ / TBZ / TBNZ）。
    JumpCond,
    /// RET。
    Ret,
    /// 其余（含间接 BR/BLR——无静态目标）。
    Other,
}

/// 分类一条 ARM64 指令：`(种类, PC 相对目标)`。
/// 间接 BR/BLR 的目标为 `None`（寄存器值静态不可知）。
pub(super) fn classify(word: u32, addr: u64) -> (ArmKind, Option<u64>) {
    let imm26 = (word & 0x03FF_FFFF) as i64;
    let sext26 = if imm26 >= 0x0200_0000 {
        imm26 - 0x0400_0000
    } else {
        imm26
    };
    let imm19 = ((word >> 5) & 0x7_FFFF) as i64;
    let sext19 = if imm19 >= 0x4_0000 {
        imm19 - 0x8_0000
    } else {
        imm19
    };
    let imm14 = ((word >> 5) & 0x3_FFFF) as i64;
    let sext14 = if imm14 >= 0x2000 {
        imm14 - 0x4000
    } else {
        imm14
    };
    let target26 = addr.wrapping_add((sext26 * 4) as u64);
    let target19 = addr.wrapping_add((sext19 * 4) as u64);
    let target14 = addr.wrapping_add((sext14 * 4) as u64);
    match word >> 26 {
        0b00_0101 => (ArmKind::Jump, Some(target26)), // B
        0b10_0101 => (ArmKind::Call, Some(target26)), // BL
        _ => match word & 0xFF00_0000 {
            0x5400_0000 => (ArmKind::JumpCond, Some(target19)), // B.cond
            _ => match (word >> 25) & 0x3F {
                // bits 31-25（sf 折叠后）：011010 = CBZ/CBNZ，011011 = TBZ/TBNZ
                0b011010 => (ArmKind::JumpCond, Some(target19)), // CBZ/CBNZ（32/64 位）
                0b011011 => (ArmKind::JumpCond, Some(target14)), // TBZ/TBNZ（32/64 位）
                _ => match word & 0xFFFF_FC1F {
                    0xD65F_0000 => (ArmKind::Ret, None), // RET
                    _ => match word & 0xFFFF_FC00 {
                        0xD63F_0000 => (ArmKind::Call, None), // BLR（间接调用）
                        0xD61F_0000 => (ArmKind::Jump, None), // BR（间接跳）
                        _ => (ArmKind::Other, None),
                    },
                },
            },
        },
    }
}

/// ARM64 块遍历：从可信种子按 4 字节步进线性走 + 控制流边扩展。
/// 纪律同 x86 侧：条件边 + 顺序边都走；无条件跳不走顺序边；RET 停链；
/// 访问数触顶停走。产出与 x86 侧同型 [`WalkOutput`]。
pub(super) fn arm64_walk(exec: &[(u64, &[u8])], seeds: &BTreeSet<u64>) -> WalkOutput {
    let mut out = WalkOutput::default();
    let mut stack: Vec<u64> = seeds
        .iter()
        .filter(|s| in_exec_any(**s, exec))
        .cloned()
        .collect();
    while let Some(addr) = stack.pop() {
        if out.visited.len() >= super::cfg::WALK_CAP {
            break;
        }
        let Some((base, data)) = section_of(addr, exec) else {
            continue;
        };
        let mut cur = addr;
        loop {
            if out.visited.contains_key(&cur) || out.visited.len() >= super::cfg::WALK_CAP {
                break;
            }
            let off = (cur - base) as usize;
            if !off.is_multiple_of(4) || off + 4 > data.len() {
                break; // 步进错位/越界：停链（ARM64 指令必须 4 对齐）
            }
            let word = u32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]]);
            let (kind, target) = classify(word, cur);
            out.visited.insert(cur, 4);
            match kind {
                ArmKind::Call => {
                    if let Some(t) = target
                        && in_exec_any(t, exec)
                        && out.call_targets.insert(t)
                    {
                        stack.push(t);
                    }
                    cur += ARM64_INSN_LEN; // call 有顺序后继
                }
                ArmKind::Jump => {
                    if let Some(t) = target
                        && in_exec_any(t, exec)
                    {
                        if t > cur {
                            out.fwd_jumps.insert(t); // 尾调用/跳板候选
                        }
                        stack.push(t);
                    }
                    break; // 无条件跳：不顺序走
                }
                ArmKind::JumpCond => {
                    if let Some(t) = target
                        && in_exec_any(t, exec)
                    {
                        stack.push(t);
                    }
                    cur += ARM64_INSN_LEN; // 条件边 + 顺序边都走
                }
                ArmKind::Ret => break,
                ArmKind::Other => cur += ARM64_INSN_LEN,
            }
            if out.visited.len() >= super::cfg::WALK_CAP {
                break;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_direct_branches() {
        // BL +4 字节（imm26=1：target = PC + 4 = 0x1004）
        let (k, t) = classify(0x9400_0001, 0x1000);
        assert_eq!(k, ArmKind::Call);
        assert_eq!(t, Some(0x1004));
        // B -4（回跳：target = PC - 4 = 0xFFC）
        let (k, t) = classify(0x17FF_FFFF, 0x1000);
        assert_eq!(k, ArmKind::Jump);
        assert_eq!(t, Some(0xFFC));
        // B +8
        let (k, t) = classify(0x1400_0002, 0x1000);
        assert_eq!(k, ArmKind::Jump);
        assert_eq!(t, Some(0x1008));
        // RET
        let (k, t) = classify(0xD65F_0000, 0x1000);
        assert_eq!(k, ArmKind::Ret);
        assert_eq!(t, None);
    }

    #[test]
    fn classify_cond_family_all_widths() {
        // B.cond（0x54）/CBZ（0x34）/CBZ 64 位（0xB4）/CBNZ（0xB5）/TBZ（0x36）
        for word in [
            0x5400_0040u32,
            0x3400_0040u32,
            0xB400_0040u32,
            0xB500_0040u32,
            0x3600_0040u32,
        ] {
            let (k, _) = classify(word, 0x1000);
            assert_eq!(k, ArmKind::JumpCond, "word {word:#x}");
        }
    }

    #[test]
    fn indirect_blr_br_have_no_static_target() {
        let (k, t) = classify(0xD63F_0000, 0x1000); // BLR x0
        assert_eq!(k, ArmKind::Call);
        assert_eq!(t, None, "间接调用无静态目标");
        let (k, t) = classify(0xD61F_0000, 0x1000); // BR x0
        assert_eq!(k, ArmKind::Jump);
        assert_eq!(t, None);
    }

    #[test]
    fn arm64_walk_follows_bl_and_stops_at_ret() {
        // 0x1000: BL → 0x100C；0x1004: RET；0x100C: RET（调用目标）
        let mut data = vec![0u8; 16];
        data[0..4].copy_from_slice(&bl_bytes(0x1000, 0x100C));
        data[4..8].copy_from_slice(&0xD65F_0000u32.to_le_bytes()); // RET @0x1004
        data[12..16].copy_from_slice(&0xD65F_0000u32.to_le_bytes()); // RET @0x100C
        let exec = vec![(0x1000u64, data.as_slice())];
        let mut seeds = std::collections::BTreeSet::new();
        seeds.insert(0x1000u64);
        let out = arm64_walk(&exec, &seeds);
        assert!(out.call_targets.contains(&0x100C), "BL 目标成为调用目标");
        assert!(out.visited.contains_key(&0x100C), "调用目标被遍历");
    }

    fn bl_bytes(from: u64, to: u64) -> [u8; 4] {
        let off = ((to as i64 - from as i64) / 4) as u32;
        (0x9400_0000 | (off & 0x03FF_FFFF)).to_le_bytes()
    }
}
