//! N2 最小 CFG（第一层）：直接控制流边收割 + 调用目标定起点 + **补隙合并**。
//!
//! 与旧 `heuristic_ranges` 的关系——起点收割语义相同（入口 + 直接 `call`
//! 目标 + 无条件直接跳转目标，全部经「落在可执行节内」校验），但多出两条纪律：
//!
//! 1. **有精确边界源时只补隙**：候选起点落在精确区间（`.pdata`/符号表/
//!    CFI）内的一律丢弃——绝不切分精确区间。往精确函数体中间插一条假起点
//!    会把一个函数劈成两半，同函数共现随之碎裂——那是 `fn_*` 判定回归的
//!    头号来源。
//! 2. **无精确边界源时覆盖全部可执行节**（旧行为的超集，语义 = 替换）。
//!
//! 已知边界（诚实记录）：这是字节模式级的边收割，不是指令级遍历——
//! 没有 x86 长度解码器，「基本块」的真实粒度只能是边与边之间的字节段；
//! 数据里的 `E8`/`E9` 会产生假边（与旧启发式同风险）。指令级块遍历留 v2，
//! 前提是最小长度解码器。因此 Cfg 边界的 `exact` 恒为 false——共现判定
//! 照旧要求精确源，N2 的「剥表 PE32 共现可判」半只在这里**保持诚实**：
//! 换个名字不改变证据等级。

use super::x86len;
use super::{FuncSource, Function};
use std::collections::{BTreeMap, BTreeSet};

use super::Arch;

/// 一条直接控制流边（字节模式收割）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FlowEdge {
    /// 边起点（指令字节的地址，PE 为 RVA / ELF 为虚拟地址）。
    pub from: u64,
    /// 边目标（跳转/调用落点）。
    pub to: u64,
    /// 是否调用边（x86 `E8` / ARM64 `BL`）。
    pub call: bool,
}

/// CFG 补隙的函数区间。
///
/// `exact` 是已归一化（升序、互不重叠）的精确边界；返回的区间只落在精确
/// 边界未覆盖的可执行区间内，[`FuncSource::Cfg`] 标注。无精确边界时
/// `exact` 传空切片——此时覆盖全部可执行节（替换旧启发式的形态）。
///
/// v2（N2）：x86 起点合成 = **块遍历验证/发现的调用目标**（可达 = 真边，
/// 遍历顺带挖出种子不可达的传递调用目标）∪ **未被遍历覆盖的层一候选**
/// （解码失败/未走到的区域回落层一——fail-closed：遍历覆盖不了就回到
/// v1 行为，绝不静默缩覆盖）。ARM64 固定 4 字节步进，层一扫描已精确，
/// 不走块遍历。
///
/// v4（N2）：`pointer_seeds` = 数据节指针表收割的候选起点（回调/虚表
/// 函数在数据节里被引用）——只当起点、不进遍历（零覆盖污染风险）。
pub(super) fn cfg_ranges(
    exec: &[(u64, &[u8])],
    exact: &[Function],
    entry: u64,
    arch: Arch,
    pointer_seeds: &[u64],
) -> Vec<Function> {
    let gaps = uncovered(exec, exact);
    if gaps.is_empty() {
        return Vec::new();
    }
    // 层一候选：入口 + 全部直接 call / 无条件跳转目标（字节级收割）。
    let mut candidates = BTreeSet::new();
    if entry != 0 {
        candidates.insert(entry);
    }
    for &(base, data) in exec {
        for edge in harvest_edges(base, data, arch) {
            candidates.insert(edge.to);
        }
    }
    // v2：x86 用块遍历做验证与传递发现。种子只取**可信起点**（入口 + 精确
    // 区间起点）——从字节候选起走会拿数据当代码，覆盖面会撒谎。
    let mut trusted = BTreeSet::new();
    if entry != 0 {
        trusted.insert(entry);
    }
    trusted.extend(exact.iter().map(|f| f.start));
    let walk_out = if arch == Arch::X86 {
        walk(exec, &trusted, arch)
    } else {
        WalkOutput::default()
    };
    // 起点合成：
    //   ① 遍历确认的调用目标（可达 = 真边，含遍历挖出的传递目标）
    //   ② 前向无条件跳目标（尾调用/跳板——遍历归类，比层一的字节猜测准）
    //   ③ 取址目标（v3：回调/虚表项——间接调用函数的起点信号）
    //   ④ 层一候选里**未被遍历覆盖**的（停走/未走到的区域 → 回落层一）
    let mut starts = BTreeSet::new();
    starts.extend(walk_out.call_targets.iter());
    starts.extend(walk_out.fwd_jumps.iter());
    starts.extend(walk_out.addr_taken.iter());
    starts.extend(pointer_seeds.iter()); // v4：数据节指针表（收割侧已过滤）
    if entry != 0 {
        starts.insert(entry); // 入口无条件是起点（隙外的由下面的 retain 挡）
    }
    for candidate in &candidates {
        if !walk_out.covers(*candidate) {
            starts.insert(*candidate);
        }
    }
    // 只收落在未覆盖区间内的起点：精确区间绝不切分。
    starts.retain(|start| in_gaps(*start, &gaps));
    // 区间 = [起点, 所在未覆盖区间末尾)，按下一起点截断（与旧平铺语义一致）。
    let mut ranges = Vec::new();
    for (low, high) in &gaps {
        for start in starts.range(*low..*high) {
            ranges.push(Function {
                start: *start,
                end: *high,
                name: None,
                source: FuncSource::Cfg,
            });
        }
    }
    for index in 0..ranges.len().saturating_sub(1) {
        let next = ranges[index + 1].start;
        if next < ranges[index].end {
            ranges[index].end = next;
        }
    }
    ranges
}

/// 块遍历的结果（x86）。
#[derive(Default)]
pub(super) struct WalkOutput {
    /// 遍历中发现的直接调用目标（可达 = 真边）。
    pub call_targets: BTreeSet<u64>,
    /// 前向无条件跳目标（尾调用/跳板候选）。
    pub fwd_jumps: BTreeSet<u64>,
    /// **取址目标**（v3）：`lea r64,[rip+disp32]` 的落点——间接调用函数的
    /// 起点信号（回调/虚表项）；遍历把它们当新种子继续走（不动点）。
    pub addr_taken: BTreeSet<u64>,
    /// 已访问指令：起点 → 长度。
    pub visited: BTreeMap<u64, usize>,
}

impl WalkOutput {
    /// 地址是否落在已遍历的某条指令内（含起点）。
    /// 覆盖判定只需查 `[addr-15, addr]`（x86 单条最长 15 字节）。
    fn covers(&self, addr: u64) -> bool {
        let low = addr.saturating_sub(15);
        self.visited
            .range(low..=addr)
            .any(|(start, len)| addr < start + *len as u64)
    }
}

/// 访问指令数上限（病态输入护栏；触顶后未走区域回落层一）。
const WALK_CAP: usize = 200_000;

/// 递归下降块遍历（x86）：从可信种子线性走 + 控制流边扩展。
/// 解码失败 = 停链（fail-closed，绝不猜）；条件跳的顺序边照走、无条件跳
/// 不走顺序边（fall-through 属于别处）、ret 停链。目标只追可执行域内的。
pub(super) fn walk(exec: &[(u64, &[u8])], seeds: &BTreeSet<u64>, arch: Arch) -> WalkOutput {
    let mut out = WalkOutput::default();
    if arch != Arch::X86 {
        // ARM64 固定 4 字节步进，层一扫描已精确，块遍历留给 v3
        return out;
    }
    let mut stack: Vec<u64> = seeds
        .iter()
        .filter(|s| in_exec_any(**s, exec))
        .cloned()
        .collect();
    while let Some(addr) = stack.pop() {
        if out.visited.len() >= WALK_CAP {
            break;
        }
        let Some((base, data)) = section_of(addr, exec) else {
            continue;
        };
        let mut cur = addr;
        loop {
            if out.visited.contains_key(&cur) || out.visited.len() >= WALK_CAP {
                break;
            }
            let Some(d) = x86len::decode(data, base, cur) else {
                break; // 停链：解不出就停，不猜
            };
            out.visited.insert(cur, d.len);
            // v3：取址目标 = 间接调用函数的起点信号——当种子继续走（不动点）
            if let Some(t) = d.addr_taken
                && in_exec_any(t, exec)
                && out.addr_taken.insert(t)
            {
                stack.push(t);
            }
            match d.kind {
                x86len::OpKind::Call => {
                    if let Some(t) = d.target
                        && in_exec_any(t, exec)
                        && out.call_targets.insert(t)
                    {
                        stack.push(t);
                    }
                    cur += d.len as u64; // call 有顺序后继
                }
                x86len::OpKind::Jump => {
                    if let Some(t) = d.target
                        && in_exec_any(t, exec)
                    {
                        if t > cur {
                            out.fwd_jumps.insert(t); // 尾调用/跳板候选
                        }
                        stack.push(t);
                    }
                    break; // 无条件跳：不顺序走
                }
                x86len::OpKind::JumpCond => {
                    if let Some(t) = d.target
                        && in_exec_any(t, exec)
                    {
                        stack.push(t);
                    }
                    cur += d.len as u64; // 条件边 + 顺序边都走
                }
                x86len::OpKind::Ret => break,
                x86len::OpKind::Other => cur += d.len as u64,
            }
        }
    }
    out
}

fn in_exec_any(addr: u64, exec: &[(u64, &[u8])]) -> bool {
    exec.iter()
        .any(|(base, data)| addr >= *base && addr < base.saturating_add(data.len() as u64))
}

fn section_of<'a>(addr: u64, exec: &'a [(u64, &'a [u8])]) -> Option<(u64, &'a [u8])> {
    exec.iter()
        .find(|(base, data)| addr >= *base && addr < base.saturating_add(data.len() as u64))
        .map(|(base, data)| (*base, *data))
}

/// 收割可执行段里的直接控制流边（字节模式，目标须落在该段内）。
///
/// x86：`E8`（call rel32）/ `E9`（jmp rel32），逐字节扫描——重叠位置也收
/// （数据里的假边是已知风险，见模块文档）。ARM64：`BL`（`0x9400_0000` 掩码）
/// / `B`（`0x1400_0000` 掩码），4 字节步进对齐扫描。
fn harvest_edges(base: u64, data: &[u8], arch: Arch) -> Vec<FlowEdge> {
    let high = base.saturating_add(data.len() as u64);
    let mut edges = Vec::new();
    match arch {
        Arch::X86 => {
            let mut index = 0usize;
            while index + 5 <= data.len() {
                let call = match data[index] {
                    0xE8 => true,
                    0xE9 => false,
                    _ => {
                        index += 1;
                        continue;
                    }
                };
                let rel = i32::from_le_bytes([
                    data[index + 1],
                    data[index + 2],
                    data[index + 3],
                    data[index + 4],
                ]);
                let from = base + index as u64;
                let to = from.wrapping_add(5).wrapping_add(rel as i64 as u64);
                if to >= base && to < high {
                    edges.push(FlowEdge { from, to, call });
                }
                index += 1;
            }
        }
        Arch::Arm64 => {
            let mut index = 0usize;
            while index + 4 <= data.len() {
                let word = u32::from_le_bytes([
                    data[index],
                    data[index + 1],
                    data[index + 2],
                    data[index + 3],
                ]);
                let opcode = word & 0xFC00_0000;
                let call = match opcode {
                    0x9400_0000 => true,
                    0x1400_0000 => false,
                    _ => {
                        index += 4;
                        continue;
                    }
                };
                let imm = sign_extend_28(((word & 0x03FF_FFFF) as i64) << 2);
                let from = base + index as u64;
                let to = from.wrapping_add(imm as u64);
                if to >= base && to < high {
                    edges.push(FlowEdge { from, to, call });
                }
                index += 4;
            }
        }
    }
    edges
}

fn sign_extend_28(value: i64) -> i64 {
    (value << 36) >> 36
}

/// 可执行节减去精确区间后的未覆盖区间（升序、互不重叠、与节边界取交）。
fn uncovered(exec: &[(u64, &[u8])], exact: &[Function]) -> Vec<(u64, u64)> {
    let mut exacts: Vec<(u64, u64)> = exact
        .iter()
        .map(|function| (function.start, function.end))
        .filter(|(start, end)| end > start)
        .collect();
    exacts.sort();
    let mut out = Vec::new();
    for &(base, data) in exec {
        let low = base;
        let high = base.saturating_add(data.len() as u64);
        let mut cursor = low;
        for &(start, end) in &exacts {
            if end <= cursor || start >= high {
                continue;
            }
            if start > cursor {
                out.push((cursor, start.min(high)));
            }
            cursor = cursor.max(end);
            if cursor >= high {
                break;
            }
        }
        if cursor < high {
            out.push((cursor, high));
        }
    }
    out
}

fn in_gaps(addr: u64, gaps: &[(u64, u64)]) -> bool {
    gaps.iter().any(|(low, high)| addr >= *low && addr < *high)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::functions::{FuncSource, Function};

    fn exact(start: u64, end: u64) -> Function {
        Function {
            start,
            end,
            name: None,
            source: FuncSource::Pdata,
        }
    }

    fn section(bytes: &[u8], base: u64) -> Vec<(u64, &[u8])> {
        vec![(base, bytes)]
    }

    fn e8(from_rel: u64, to: u64) -> [u8; 5] {
        // rel32 = to - (from + 5)
        let rel = (to as i64 - (from_rel as i64 + 5)) as i32;
        let mut out = [0xE8, 0, 0, 0, 0];
        out[1..].copy_from_slice(&rel.to_le_bytes());
        out
    }

    #[test]
    fn no_exact_source_covers_whole_exec_like_old_heuristic() {
        // 入口 0x1000 + E8 目标 0x1030 → 两条 Cfg 区间平铺到节末
        let mut bytes = vec![0x90u8; 0x40];
        bytes[0x10..0x15].copy_from_slice(&e8(0x10, 0x30));
        let ranges = cfg_ranges(&section(&bytes, 0x1000), &[], 0x1000, super::Arch::X86, &[]);
        let starts: Vec<u64> = ranges.iter().map(|r| r.start).collect();
        assert_eq!(starts, vec![0x1000, 0x1030], "入口 + E8 目标都成起点");
        assert!(
            ranges.iter().all(|r| r.source == FuncSource::Cfg),
            "无精确源 = 替换形态，全部 Cfg"
        );
        assert_eq!(ranges.last().unwrap().end, 0x1040, "末区间到节末");
    }

    #[test]
    fn exact_ranges_are_never_split_by_cfg_candidates() {
        // E8 目标 0x1060 落在精确区间 [0x1000,0x1100) 内 → 丢弃不切分；
        // 隙内另有 E8 目标 0x1210 → 正常补隙。两条都要成立才有判别力。
        let mut bytes = vec![0x90u8; 0x300];
        bytes[0x50..0x55].copy_from_slice(&e8(0x50, 0x60)); // 目标 0x1060 在精确区间内
        bytes[0x200..0x205].copy_from_slice(&e8(0x200, 0x210)); // 目标 0x1210 在隙内
        let exacts = vec![exact(0x1000, 0x1100)];
        let ranges = cfg_ranges(
            &section(&bytes, 0x1000),
            &exacts,
            0x1000,
            super::Arch::X86,
            &[],
        );
        assert!(
            ranges.iter().all(|r| r.start >= 0x1100),
            "Cfg 区间不得进入精确区间内部：{ranges:?}"
        );
        assert_eq!(
            ranges.iter().map(|r| r.start).collect::<Vec<_>>(),
            vec![0x1210],
            "隙内的真起点照收"
        );
    }

    #[test]
    fn cfg_fills_gaps_between_exact_ranges() {
        // 精确区间 [0x1000,0x1100)：叶子函数起点 0x1200 是 E8 目标（数据在
        // 精确区间外扫描到）→ Cfg 补隙 [0x1200, 0x1200+n)
        let mut bytes = vec![0x90u8; 0x300];
        bytes[0x200..0x205].copy_from_slice(&e8(0x200, 0x210)); // 目标 0x1210
        let exacts = vec![exact(0x1000, 0x1100)];
        let ranges = cfg_ranges(
            &section(&bytes, 0x1000),
            &exacts,
            0x1000,
            super::Arch::X86,
            &[],
        );
        assert_eq!(ranges.len(), 1, "只补一条：{ranges:?}");
        assert_eq!(ranges[0].start, 0x1210, "E8 目标 0x1210 成补隙起点");
        assert_eq!(ranges[0].end, 0x1300, "平铺到节末（无下一起点）");
        assert_eq!(ranges[0].source, FuncSource::Cfg);
    }

    #[test]
    fn entry_inside_gap_becomes_start_entry_inside_exact_is_dropped() {
        // 入口落在精确区间内 → 不重复立起点；落在隙内 → 立起点
        let bytes = vec![0x90u8; 0x40];
        let exacts = vec![exact(0x1010, 0x1020)];
        let ranges = cfg_ranges(
            &section(&bytes, 0x1000),
            &exacts,
            0x1014,
            super::Arch::X86,
            &[],
        );
        assert!(
            !ranges.iter().any(|r| r.start == 0x1014),
            "入口在精确区间内：不立 Cfg 起点"
        );
        let exacts2 = vec![exact(0x1010, 0x1020)];
        let ranges2 = cfg_ranges(
            &section(&bytes, 0x1000),
            &exacts2,
            0x1030,
            super::Arch::X86,
            &[],
        );
        assert!(
            ranges2.iter().any(|r| r.start == 0x1030),
            "入口在隙内：立 Cfg 起点"
        );
    }

    #[test]
    fn arm64_bl_targets_seed_starts() {
        // BL 0x14000000 | imm26 → 目标 0x1010：4 字节步进，目标在节内
        let mut bytes = vec![0x00u8; 0x40];
        let word: u32 = 0x9400_0000 | ((0x10 >> 2) & 0x03FF_FFFF);
        bytes[0..4].copy_from_slice(&word.to_le_bytes());
        let ranges = cfg_ranges(&section(&bytes, 0x1000), &[], 0, super::Arch::Arm64, &[]);
        assert!(
            ranges.iter().any(|r| r.start == 0x1010),
            "BL 目标 0x1010 成起点：{ranges:?}"
        );
    }

    #[test]
    fn addr_taken_seeds_indirectly_called_leaf() {
        // 入口：lea rax,[rip+→0x1040]（取回调地址）+ call rax（间接）+ ret。
        // 叶子 0x1040 没有任何 E8 指向——层一字节收割永远看不见它，
        // v3 的取址信号把它立成起点（间接调用叶子漏检的根治路径）。
        let mut bytes = vec![0x90u8; 0x60];
        let rel = (0x1040i64 - 0x1007) as i32; // lea 在 0x1000，len=7，下一条 = 0x1007
        bytes[0] = 0x48;
        bytes[1] = 0x8D;
        bytes[2] = 0x05;
        bytes[3..7].copy_from_slice(&rel.to_le_bytes());
        bytes[7] = 0xFF; // call rax（间接，无静态目标）
        bytes[8] = 0xD0;
        bytes[9] = 0xC3;
        // 叶子：call [rip+0]（IAT 形态，decode 不解间接目标）+ ret
        bytes[0x40] = 0xFF;
        bytes[0x41] = 0x15;
        bytes[0x42..0x46].copy_from_slice(&0i32.to_le_bytes());
        bytes[0x46] = 0xC3;
        let ranges = cfg_ranges(&section(&bytes, 0x1000), &[], 0x1000, super::Arch::X86, &[]);
        let starts: Vec<u64> = ranges.iter().map(|r| r.start).collect();
        assert!(starts.contains(&0x1000), "入口照常是起点：{starts:?}");
        assert!(
            starts.contains(&0x1040),
            "取址目标 0x1040 成起点——层一看不见的间接调用叶子被 v3 捡回：{starts:?}"
        );
        assert_eq!(starts.len(), 2, "无多余候选（FF D0 间接边不产字节候选）");
    }

    #[test]
    fn pointer_table_seeds_start_vtable_functions() {
        // v4：数据节指针表收割的候选喂给 cfg_ranges → vtable 函数长出起点。
        // 叶子 A（0x1010）/B（0x1030）无任何 E8 指向——层一看不见，v4 捡回。
        let mut bytes = vec![0x90u8; 0x80];
        bytes[0x10] = 0xC3;
        bytes[0x30] = 0xC3;
        let exec = vec![(0x1000u64, bytes.as_slice())];
        let seeds = [0x1010u64, 0x1030u64];
        let ranges = cfg_ranges(&exec, &[], 0x1000, super::Arch::X86, &seeds);
        let starts: Vec<u64> = ranges.iter().map(|r| r.start).collect();
        assert!(starts.contains(&0x1010), "表里的叶子 A 成起点：{starts:?}");
        assert!(starts.contains(&0x1030), "表里的叶子 B 成起点：{starts:?}");
        // 对照：不喂种子时叶子不可见（正是 v4 要补的洞）
        let plain = cfg_ranges(&exec, &[], 0x1000, super::Arch::X86, &[]);
        assert!(
            !plain.iter().any(|r| r.start == 0x1010 || r.start == 0x1030),
            "无种子时 vtable 函数不可见"
        );
    }
}
