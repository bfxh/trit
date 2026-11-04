//! x86-64 指令长度解码器（N2 v2 的地基，最小集，**fail-closed**）。
//!
//! 回答一个问题：从字节流的某个位置起，这条指令多长？——有了它，
//! CFG 才能做指令级块遍历（真基本块），而不是字节模式收割。
//!
//! 纪律：**解不出就返回 `None`，绝不猜长度**。走块遍历时 `None` = 停走
//! （这条链进不去），不会把数据当指令继续走。已知不支持面如实列出：
//! VEX/EVEX（`C4`/`C5`/`62`）、三字节逃逸（`0F 38`/`0F 3A`）、64 位下非法
//! 的 32 位时代指令（`60`/`61`/`D4`/`D5`/`27`/`2F`/`37`/`3F`/`06`/`07`/
//! `0E`/`16`/`17`/`1E`/`1F`/`EA`/`0F A0`/`0F A1`/`0F A8`/`0F A9`）、
//! moffs 绝对寻址（`A0`-`A3`）——这些在编译器产物里几乎只出现在**数据**
//! 里，把它们当停走信号恰好是好事。
//!
//! 覆盖面：前缀（legacy + REX，REX.W 压过 `0x66` 的立即数宽度）、
//! ModRM/SIB/disp/imm 全寻址形态、主流整数指令、常用 SSE（两字节 opcode
//! 默认 ModRM 形态）、全部控制流（`E8`/`E9`/`EB`/`70-7F`/`0F 80-8F`/
//! `C3`/`C2`/`C9`/间接 `FF`）。长度上限 15 字节，超出即停走。

/// 从 `bytes[pos..]` 解出一条 x86-64 指令的长度（含前缀与立即数）。
/// 解不出（不支持/非法/超长/缓冲区装不下这条指令）返回 `None`——
/// 调用方应当停走，不猜。
pub(super) fn instr_len(bytes: &[u8], pos: usize) -> Option<usize> {
    instr_len_raw(bytes, pos)
        .filter(|len| (1..=15).contains(len))
        .filter(|len| pos + len <= bytes.len())
}

/// 控制流分类（块遍历用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OpKind {
    /// 直接调用（`E8` rel32/rel16）。
    Call,
    /// 无条件跳转（`E9`/`EB`）。
    Jump,
    /// 条件跳转（`70-7F`/`0F 80-8F`）。
    JumpCond,
    /// 返回（`C3`/`C2`/`C9`）。
    Ret,
    /// 其余。
    Other,
}

/// 一次解码：长度 + 控制流分类 + 直接边目标（非控制流或间接边为 `None`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Decoded {
    pub len: usize,
    pub kind: OpKind,
    /// 直接边的目标地址（本流内偏移 + pos）；间接边/非控制流为 `None`。
    pub target: Option<u64>,
    /// **取址目标**（v3）：`lea r64,[rip+disp32]` 的落点——地址被取 = 回调/
    /// 虚表项，间接调用函数的起点信号；非取址指令为 `None`。
    pub addr_taken: Option<u64>,
}

/// 解码一条指令并给出控制流分类（块遍历的一次原语）。
/// `base` 是 `bytes[0]` 的绝对地址（PE 为 RVA / ELF 为虚拟地址），
/// `addr` 是待解码指令的绝对地址——返回的 target 与调用方在同一地址空间。
pub(super) fn decode(bytes: &[u8], base: u64, addr: u64) -> Option<Decoded> {
    let pos = addr.checked_sub(base)? as usize;
    let len = instr_len(bytes, pos)?;
    let rest = &bytes[pos..pos + len];
    // 跳过前缀找 opcode（与 instr_len_raw 的前缀规则一致）
    let mut j = 0usize;
    while j < rest.len() {
        let b = rest[j];
        if matches!(
            b,
            0xF0 | 0xF2 | 0xF3 | 0x2E | 0x36 | 0x3E | 0x26 | 0x64 | 0x65 | 0x66 | 0x67
        ) || (0x40..=0x4F).contains(&b)
        {
            j += 1;
        } else {
            break;
        }
    }
    let op = *rest.get(j)?;
    let op2 = rest.get(j + 1).copied();
    // rel 宽度：0x66 前缀缩成 16 位（只认首个前缀字节的简化口径，模块文档已记）
    let rel_width = if rest[0] == 0x66 { 2 } else { 4 };
    let rel_at = |tail: usize| -> Option<i64> {
        let bytes_rel = rest.get(tail..tail + rel_width)?;
        let raw = match rel_width {
            2 => i16::from_le_bytes([bytes_rel[0], bytes_rel[1]]) as i64,
            _ => {
                i32::from_le_bytes([bytes_rel[0], bytes_rel[1], bytes_rel[2], bytes_rel[3]]) as i64
            }
        };
        Some(raw)
    };
    let rel_target = |tail: usize| -> Option<u64> {
        let rel = rel_at(tail)?;
        Some((addr as i64 + len as i64 + rel) as u64)
    };
    let (kind, target) = match op {
        0xE8 => (OpKind::Call, rel_target(j + 1)),
        0xE9 => (OpKind::Jump, rel_target(j + 1)),
        0xEB => (OpKind::Jump, rel_target(j + 1)),
        0x70..=0x7F => (OpKind::JumpCond, rel_target(j + 1)),
        0x0F if op2.is_some_and(|b| (0x80..=0x8F).contains(&b)) => {
            (OpKind::JumpCond, rel_target(j + 2))
        }
        0xC3 | 0xC9 => (OpKind::Ret, None),
        0xC2 => (OpKind::Ret, None),
        0xFF if op2.is_some() => {
            // FF /2 = call r/m、FF /4 = jmp r/m：间接边，无静态目标
            let modrm = op2?;
            let reg = (modrm >> 3) & 7;
            match reg {
                2 => (OpKind::Call, None),
                4 => (OpKind::Jump, None),
                _ => (OpKind::Other, None),
            }
        }
        _ => (OpKind::Other, None),
    };
    // 取址信号（v3）：LEA r64/r32,[rip+disp32]——mod=00 rm=101。
    // `0x66` 前缀缩立即数不缩 disp32；disp 恒 4 字节（67 寻址前缀不支持，见模块文档）。
    let addr_taken = if op == 0x8D {
        let modrm = op2?;
        if (modrm >> 6) == 0 && (modrm & 7) == 5 {
            rel_target(j + 2)
        } else {
            None
        }
    } else {
        None
    };
    Some(Decoded {
        len,
        kind,
        target,
        addr_taken,
    })
}

fn instr_len_raw(bytes: &[u8], pos: usize) -> Option<usize> {
    let mut i = pos;
    let mut rex_w = false;
    let mut had_66 = false;
    let mut had_f3 = false;
    // 前缀段：legacy 前缀可任意重复，REX 在其后（顺序不严格也能解长）。
    loop {
        let b = *bytes.get(i)?;
        match b {
            0xF0 | 0xF2 | 0x2E | 0x36 | 0x3E | 0x26 | 0x64 | 0x65 => i += 1,
            0xF3 => {
                had_f3 = true;
                i += 1;
            }
            0x66 => {
                had_66 = true;
                i += 1;
            }
            0x67 => i += 1,
            0x40..=0x4F => {
                rex_w = b & 0x08 != 0;
                i += 1;
            }
            _ => break,
        }
    }
    let op = *bytes.get(i)?;
    let len = i + 1 - pos;

    // 32 位立即/相对宽度：REX.W 压过 0x66；imm8 类不受影响。
    let imm32 = if had_66 { 2 } else { 4 };

    if op == 0x0F {
        let op2 = *bytes.get(i + 1)?;
        let len2 = len + 1;
        return match op2 {
            // 三字节逃逸 / VEX 通道：不支持，停走
            0x38 | 0x3A => None,
            // 无 ModRM：syscall/sysret/ud2/invd/wbinvd/clts/rdtsc/rdmsr/rdpmc/
            // sysenter/sysexit/emms/bswap/rsm/fwait
            0x05
            | 0x07
            | 0x08
            | 0x09
            | 0x0B
            | 0x30
            | 0x31
            | 0x32
            | 0x33
            | 0x34
            | 0x35
            | 0x77
            | 0xAA
            | 0xC8..=0xCF => Some(len2),
            // endbr64/endbr32（F3 0F 1E FA）：4 字节定长、无 ModRM
            0x1E if had_f3 => Some(len2 + 1),
            // 64 位下非法的 32 位时代指令：停走信号（数据里的概率远高于代码）
            0x04 | 0x0A | 0x24 | 0x26 | 0x27 | 0x36 | 0x37 | 0xA0 | 0xA1 | 0xA8 | 0xA9 => None,
            // jcc rel32（0x66 缩成 rel16）
            0x80..=0x8F => Some(len2 + imm32),
            // 带 imm8 的两字节形态
            0xA4 | 0xAC | 0x70 | 0x71 | 0x72 | 0x73 | 0xBA | 0xC2 => {
                modrm_len(bytes, i + 2).map(|m| len2 + m + 1)
            }
            // 其余两字节 opcode 默认 ModRM 形态（编译器产物里绝大多数如此）
            _ => modrm_len(bytes, i + 2).map(|m| len2 + m),
        };
    }

    match op {
        // 控制流：rel32 / rel8（0x66 缩 rel16）
        0xE8 | 0xE9 => Some(len + imm32),
        0xEB | 0x70..=0x7F => Some(len + 1),
        // 无操作数：ret/leave/nop/标志位/int3/cwde/cdq/pushf/popf/sahf/lahf
        0xC3
        | 0xC9
        | 0x90
        | 0x98
        | 0x99
        | 0x9B
        | 0x9C
        | 0x9D
        | 0x9E
        | 0x9F
        | 0xF1
        | 0xF5
        | 0xF8..=0xFD => Some(len),
        0xCC | 0xCE | 0xCF => Some(len),
        0xC2 | 0xCA => Some(len + 2), // ret/retf imm16
        // push/pop imm
        0x68 => Some(len + imm32),
        0x6A => Some(len + 1),
        // test al/eax,imm
        0xA8 => Some(len + 1),
        0xA9 => Some(len + imm32),
        // mov r8,imm8 / mov r,imm（REX.W 下 B8-BF 是 imm64——movabs）
        0xB0..=0xB7 => Some(len + 1),
        0xB8..=0xBF => {
            let imm = if rex_w {
                8
            } else if had_66 {
                2
            } else {
                4
            };
            Some(len + imm)
        }
        // in/out imm8 与 in/out dx
        0xE4..=0xE7 => Some(len + 1),
        0xEC..=0xEF => Some(len),
        // loop/jrcxz rel8
        0xE0..=0xE3 => Some(len + 1),
        // imul r,r/m,imm
        0x69 => modrm_len(bytes, i + 1).map(|m| len + m + imm32),
        0x6B => modrm_len(bytes, i + 1).map(|m| len + m + 1),
        // group1 imm（0x82 是 0x80 的别名）
        0x80 | 0x82 => modrm_len(bytes, i + 1).map(|m| len + m + 1),
        0x81 => modrm_len(bytes, i + 1).map(|m| len + m + imm32),
        0x83 => modrm_len(bytes, i + 1).map(|m| len + m + 1),
        // shift 组 imm8
        0xC0 | 0xC1 => modrm_len(bytes, i + 1).map(|m| len + m + 1),
        // mov r/m8,imm8 / mov r/m,imm32
        0xC6 => modrm_len(bytes, i + 1).map(|m| len + m + 1),
        0xC7 => modrm_len(bytes, i + 1).map(|m| len + m + imm32),
        // test/not/neg/mul/imul/div/idiv：reg 字段 0/1 才带 imm
        0xF6 => {
            let m = modrm_len(bytes, i + 1)?;
            let reg = (bytes.get(i + 1)? >> 3) & 7;
            let imm = usize::from(reg <= 1);
            Some(len + m + imm)
        }
        0xF7 => {
            let m = modrm_len(bytes, i + 1)?;
            let reg = (bytes.get(i + 1)? >> 3) & 7;
            let imm = if reg <= 1 { imm32 } else { 0 };
            Some(len + m + imm)
        }
        // 64 位下非法的 32 位时代指令 / moffs / VEX：停走信号
        0x06
        | 0x07
        | 0x0E
        | 0x16
        | 0x17
        | 0x1E
        | 0x1F
        | 0x27
        | 0x2F
        | 0x37
        | 0x3F
        | 0x60
        | 0x61
        | 0xD4
        | 0xD5
        | 0xEA
        | 0xA0..=0xA3
        | 0xC4
        | 0xC5
        | 0x62 => None,
        // x87（D8-DF）、FE/FF 组、00-3F 算术、84-8F 传送/lea、D0-D3 移位
        _ => {
            if (0xAA..=0xAF).contains(&op) || (0x50..=0x5F).contains(&op) {
                return Some(len);
            }
            if (0x00..=0x3F).contains(&op)
                || (0x84..=0x8F).contains(&op)
                || (0xD0..=0xD3).contains(&op)
                || (0xD8..=0xDF).contains(&op)
                || op == 0xFE
                || op == 0xFF
            {
                return modrm_len(bytes, i + 1).map(|m| len + m);
            }
            // 其余冷门 opcode → ModRM 默认（编译器产物里绝大多数如此）
            modrm_len(bytes, i + 1).map(|m| len + m)
        }
    }
}

/// ModRM 字节在 `bytes[pos]`，返回 ModRM + SIB + disp 的字节数（不含指令码）。
fn modrm_len(bytes: &[u8], pos: usize) -> Option<usize> {
    let modrm = *bytes.get(pos)?;
    let (mod_bits, rm) = (modrm >> 6, modrm & 7);
    if mod_bits == 3 {
        return Some(1);
    }
    let mut len = 1usize;
    if rm == 4 {
        // SIB：base = SIB & 7；base=101 且 mod=00 → 无基址 disp32
        let sib = *bytes.get(pos + 1)?;
        len += 1;
        if sib & 7 == 5 && mod_bits == 0 {
            return Some(len + 4);
        }
    } else if rm == 5 && mod_bits == 0 {
        // RIP 相对 disp32
        return Some(len + 4);
    }
    len += match mod_bits {
        1 => 1,
        2 => 4,
        _ => 0,
    };
    Some(len)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn l(bytes: &[u8]) -> usize {
        instr_len(bytes, 0).expect("应解出")
    }

    #[test]
    fn no_operand_and_common_integer_forms() {
        assert_eq!(l(&[0x90]), 1, "nop");
        assert_eq!(l(&[0xC3]), 1, "ret");
        assert_eq!(l(&[0x48, 0x89, 0xE5]), 3, "mov rbp,rsp（reg,reg 无 disp）");
        assert_eq!(l(&[0x31, 0xC0]), 2, "xor eax,eax");
        assert_eq!(l(&[0x50]), 1, "push rax");
        assert_eq!(l(&[0x5D]), 1, "pop rbp");
        assert_eq!(
            l(&[0x48, 0x83, 0xC4, 0x08]),
            4,
            "add rsp,8（REX+op+modrm+imm8）"
        );
        assert_eq!(
            l(&[0x48, 0x81, 0xEC, 0x00, 0x01, 0x00, 0x00]),
            7,
            "sub rsp,imm32"
        );
        assert_eq!(l(&[0x83, 0xC4, 0x08]), 3, "add eax,imm8");
        assert_eq!(l(&[0xB8, 0x01, 0x00, 0x00, 0x00]), 5, "mov eax,imm32");
        assert_eq!(
            l(&[0x48, 0xC7, 0xC0, 0x01, 0x00, 0x00, 0x00]),
            7,
            "mov rax,imm32（REX.W 不扩 C7 的 imm）"
        );
        assert_eq!(
            l(&[0x48, 0xB8, 0x01, 0, 0, 0, 0, 0, 0, 0, 0]),
            10,
            "movabs rax,imm64"
        );
        assert_eq!(l(&[0x68, 0x01, 0, 0, 0]), 5, "push imm32");
        assert_eq!(l(&[0x6A, 0x10]), 2, "push imm8");
        assert_eq!(l(&[0xB0, 0x10]), 2, "mov al,imm8");
    }

    #[test]
    fn modrm_sib_and_rip_relative() {
        assert_eq!(
            l(&[0x48, 0x8B, 0x05, 1, 2, 3, 4]),
            7,
            "mov rax,[rip+disp32]"
        );
        assert_eq!(l(&[0x48, 0x8D, 0x48, 0x10]), 4, "lea rcx,[rax+disp8]");
        assert_eq!(
            l(&[0x48, 0x8D, 0x80, 1, 2, 3, 4]),
            7,
            "lea rax,[rax+disp32]"
        );
        assert_eq!(
            l(&[0x48, 0x8D, 0x64, 0x24, 0x08]),
            5,
            "lea rsp,[rsp+disp8]（SIB）"
        );
        assert_eq!(
            l(&[0x8B, 0x04, 0x25, 1, 2, 3, 4]),
            7,
            "mov eax,[disp32]（SIB base=101,mod=00）"
        );
        assert_eq!(l(&[0x48, 0x8B, 0x44, 0x24, 0x10]), 5, "mov rax,[rsp+disp8]");
        assert_eq!(l(&[0x8A, 0x00]), 2, "mov al,[rax]");
        assert_eq!(
            l(&[0x66, 0x48, 0x8B, 0x40, 0x10]),
            5,
            "前缀叠加不改 ModRM 长度"
        );
    }

    #[test]
    fn control_flow_lengths() {
        assert_eq!(l(&[0xE8, 0, 0, 0, 0]), 5, "call rel32");
        assert_eq!(l(&[0xE9, 0, 0, 0, 0]), 5, "jmp rel32");
        assert_eq!(l(&[0xEB, 0x10]), 2, "jmp rel8");
        assert_eq!(l(&[0x74, 0x10]), 2, "jcc rel8");
        assert_eq!(l(&[0x0F, 0x84, 0, 0, 0, 0]), 6, "jcc rel32");
        assert_eq!(l(&[0x66, 0xE8, 0, 0]), 4, "call rel16（0x66 前缀缩 rel）");
        assert_eq!(l(&[0xC2, 0x10, 0x00]), 3, "ret imm16");
        assert_eq!(l(&[0xFF, 0x15, 1, 2, 3, 4]), 6, "call [rip+disp32]");
        assert_eq!(l(&[0xFF, 0xE0]), 2, "jmp rax（间接）");
        assert_eq!(l(&[0xFF, 0x14, 0x24]), 3, "call [rsp]（SIB）");
        assert_eq!(l(&[0xE0, 0x10]), 2, "loopne rel8");
        assert_eq!(l(&[0xE3, 0x00]), 2, "jrcxz rel8");
    }

    #[test]
    fn sse_prefix_and_endbr() {
        assert_eq!(l(&[0xF3, 0x0F, 0x1E, 0xFA]), 4, "endbr64");
        assert_eq!(l(&[0x66, 0x0F, 0x6E, 0xC0]), 4, "movd xmm0,eax");
        assert_eq!(l(&[0xF3, 0x0F, 0x7F, 0x00]), 4, "movdqu [rax],xmm0");
        assert_eq!(
            l(&[0x0F, 0x28, 0x44, 0x24, 0x10]),
            5,
            "movaps xmm0,[rsp+disp8]"
        );
        assert_eq!(
            l(&[0x0F, 0x1F, 0x00]),
            3,
            "nop r/m（无 F3 的 0F1F 是 ModRM 形态）"
        );
        assert_eq!(l(&[0x0F, 0xAF, 0xC1]), 3, "imul eax,ecx");
        assert_eq!(
            l(&[0x0F, 0xB6, 0x44, 0x24, 0x10]),
            5,
            "movzx eax,[rsp+disp8]"
        );
        assert_eq!(l(&[0x0F, 0xBA, 0xE0, 0x10]), 4, "bt r/m,imm8");
    }

    #[test]
    fn fail_closed_on_unsupported_and_invalid() {
        assert_eq!(
            instr_len(&[0xC4, 0xE2, 0x7D, 0x00], 0),
            None,
            "VEX 三字节 → 停走"
        );
        assert_eq!(instr_len(&[0xC5, 0xF8, 0x10], 0), None, "VEX 两字节 → 停走");
        assert_eq!(instr_len(&[0x62, 0xF1], 0), None, "EVEX → 停走");
        assert_eq!(instr_len(&[0x0F, 0x38, 0x00], 0), None, "三字节逃逸 → 停走");
        assert_eq!(instr_len(&[0x0F, 0x3A, 0x00], 0), None, "三字节逃逸 → 停走");
        assert_eq!(instr_len(&[0x60], 0), None, "pusha 在 64 位非法 → 停走");
        assert_eq!(instr_len(&[0xD4, 0x0A], 0), None, "aam 在 64 位非法 → 停走");
        assert_eq!(
            instr_len(&[0xEA, 0, 0, 0, 0, 0, 0], 0),
            None,
            "远跳 64 位非法 → 停走"
        );
        assert_eq!(
            instr_len(&[0x0F, 0xA0], 0),
            None,
            "push fs 在 64 位非法 → 停走"
        );
        assert_eq!(
            instr_len(&[0xA0, 1, 2, 3, 4, 5, 6, 7, 8], 0),
            None,
            "moffs → 停走"
        );
        assert_eq!(instr_len(&[0x48], 0), None, "截断在 opcode → 停走");
        assert_eq!(instr_len(&[0x48, 0x8B], 0), None, "截断在 ModRM → 停走");
        assert_eq!(
            instr_len(&[0x48, 0x8B, 0x80, 1, 2], 0),
            None,
            "截断在 disp32 → 停走"
        );
        assert_eq!(instr_len(&[], 0), None, "空流 → 停走");
    }

    #[test]
    fn f7_group_imm_only_for_reg_0_and_1() {
        assert_eq!(
            l(&[0xF7, 0xC0, 1, 2, 3, 4]),
            6,
            "test rax,imm32（reg=0 带 imm）"
        );
        assert_eq!(l(&[0xF7, 0xE0]), 2, "mul rax（reg=4 无 imm）");
        assert_eq!(l(&[0xF7, 0xE8]), 2, "imul rax（reg=5 无 imm）");
        assert_eq!(l(&[0xF6, 0xC8, 0x7F]), 3, "test r/m8,imm8（reg=1 带 imm8）");
    }

    #[test]
    fn length_capped_at_fifteen() {
        // 14 个段前缀 + modrm disp32 = 20 > 15 → 停走而不是给假长度
        let mut bytes = vec![0x2Eu8; 14];
        bytes.extend([0x8B, 0x05, 1, 2, 3, 4]);
        assert_eq!(instr_len(&bytes, 0), None, "超过 15 字节上限 → 停走");
    }

    #[test]
    fn decode_from_offset_positions() {
        let bytes = [0x50, 0xC3, 0x48, 0x89, 0xE5, 0xC3];
        assert_eq!(instr_len(&bytes, 0), Some(1));
        assert_eq!(instr_len(&bytes, 2), Some(3), "从中间位置解");
        assert_eq!(instr_len(&bytes, 5), Some(1));
    }

    #[test]
    fn addr_taken_only_for_lea_rip_relative() {
        // lea rax,[rip+0x20] @0x1000：len=7，target = 0x1000+7+0x20
        let bytes = [0x48, 0x8D, 0x05, 0x20, 0x00, 0x00, 0x00, 0xC3];
        let d = decode(&bytes, 0x1000, 0x1000).expect("解出");
        assert_eq!(d.len, 7);
        assert_eq!(
            d.addr_taken,
            Some(0x1027),
            "lea [rip+disp] 的落点 = 取址信号"
        );
        // mov rax,[rip+disp]：加载不取址
        let mov = [0x48, 0x8B, 0x05, 0x20, 0x00, 0x00, 0x00, 0xC3];
        let d = decode(&mov, 0x1000, 0x1000).expect("解出");
        assert_eq!(d.addr_taken, None, "mov 是加载不是取址");
        // lea rcx,[rax+disp8]：非 rip 相对
        let nonrel = [0x48, 0x8D, 0x48, 0x10, 0xC3];
        let d = decode(&nonrel, 0x1000, 0x1000).expect("解出");
        assert_eq!(d.addr_taken, None);
        // lea rax,[rax+disp32]：mod=10 非取址形态
        let disp = [0x48, 0x8D, 0x80, 1, 2, 3, 4, 0xC3];
        let d = decode(&disp, 0x1000, 0x1000).expect("解出");
        assert_eq!(d.addr_taken, None);
        // 控制流指令的 addr_taken 恒空
        let call = [0xE8, 0x10, 0x00, 0x00, 0x00, 0xC3];
        let d = decode(&call, 0x1000, 0x1000).expect("解出");
        assert_eq!(d.addr_taken, None);
    }
}
