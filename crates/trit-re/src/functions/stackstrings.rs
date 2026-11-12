//! 栈串提取（dynres 的下一层证据源）：loader 把敏感 API 名**逐字节写上栈**
//!（`mov byte ptr [rsp+X], 'V'` 序列）——数据段扫描对它全盲（字面串不在
//! 任何数据节里）。本模块在**可执行节切片**上做线性扫（x86len 步长），
//! 收集「mov 立即数进内存」与「REX.W mov r64, imm64」的立即数字节，
//! 按地址顺序拼接成流，再在流里找敏感名子串。
//!
//! 诚实边界（v1）：线性扫在嵌数据上会错位（错位产生的 c6/c7 假立即数
//! 由「≥2 个不同敏感名」阈值兜底）；只覆盖 x86-64 编码（ARM64 的
//! movz/movk 半字形式待扩）；v1 不处理 disp32+SIB base=5 的罕见形态。
//! 证据**只进 `Unknown`/`True` 判定，绝不参与 False 结论**（误对齐不
//! 等于"没有"）。

use super::x86len::instr_len;

/// 前缀字节（legacy + REX）——跳过取 opcode；REX.W 记下（C7 宽度语义）。
fn opcode_start(bytes: &[u8], pos: usize) -> Option<(usize, bool)> {
    let mut p = pos;
    let mut rex_w = false;
    let mut n = 0;
    while p < bytes.len() && n < 4 {
        let b = bytes[p];
        if matches!(
            b,
            0x66 | 0x67 | 0xF2 | 0xF3 | 0x2E | 0x3E | 0x26 | 0x36 | 0x64 | 0x65
        ) {
            p += 1;
            n += 1;
            continue;
        }
        if (0x40..0x50).contains(&b) {
            rex_w = b & 0x08 != 0;
            p += 1;
            n += 1;
            continue;
        }
        return Some((p, rex_w));
    }
    None
}

/// 单条指令的立即数（mov 立即数形态）；非该形态 → `None`。
/// 认两种：①`C6/C7 /0`（mov r/m8/32, imm——写内存拼栈串的主力）；
/// ②`B8+r`（mov r32/64, imm——先入寄存器再转存的前半段）。
fn mov_imm_bytes(bytes: &[u8], pos: usize) -> Option<Vec<u8>> {
    let (op, rex_w) = opcode_start(bytes, pos)?;
    let b0 = *bytes.get(op)?;
    // ②寄存器立即数：B8+r（REX.W → imm64，否则 imm32）
    if (0xB8..=0xBF).contains(&b0) {
        let take = if rex_w { 8 } else { 4 };
        return Some(bytes.get(op + 1..op + 1 + take)?.to_vec());
    }
    // ①内存立即数：C6/C7 /0
    if b0 != 0xC6 && b0 != 0xC7 {
        return None;
    }
    let modrm = *bytes.get(op + 1)?;
    if modrm & 0x38 != 0 {
        return None; // 只认 /0（mov r/m, imm）
    }
    let (mod_, rm) = (modrm >> 6, modrm & 7);
    if mod_ == 3 {
        return None; // 寄存器目标：不是写内存
    }
    let disp: usize = match mod_ {
        1 => 1,
        2 => 4,
        _ => usize::from(rm == 5), // mod00 rm101 = disp32；rm100(SIB base=5) 罕见 v1 不认
    };
    let sib = usize::from(rm == 4 && mod_ != 0);
    let imm_off = op + 2 + sib + disp;
    let take = match b0 {
        0xC6 => 1,
        0xC7 => {
            if rex_w {
                8 // REX.W C7 = mov r64, imm32 符号扩展——取 imm32 的 4 字节
            } else {
                4
            }
        }
        _ => return None,
    };
    Some(bytes.get(imm_off..imm_off + take)?.to_vec())
}

/// 在可执行节切片上做线性扫，返回按地址顺序拼接的立即数字节流。
fn immediate_stream(slices: &[&[u8]]) -> Vec<u8> {
    let mut stream = Vec::new();
    for slice in slices {
        let mut pos = 0usize;
        while pos < slice.len() {
            let rest = &slice[pos..];
            match instr_len(rest, 0) {
                Some(len) => {
                    stream.extend(mov_imm_bytes(rest, 0).unwrap_or_default());
                    pos += len;
                }
                None => pos += 1, // 解不出：单步滑过（数据/不支持形态）
            }
        }
    }
    stream
}

/// 栈串证据：立即数流里出现的敏感名子串（去重、按词汇表顺序）。
/// 阈值判断（≥2 个不同名 = True）由调用方做——本函数只报命中。
pub fn stack_string_hits(slices: &[&[u8]], vocab: &[&'static str]) -> Vec<&'static str> {
    let stream = immediate_stream(slices);
    search_stream(&stream, vocab)
}

/// **ARM64 栈串流**：AArch64 无立即数进内存指令——串在栈上靠
/// `movz/movk`（16 位半字入寄存器）+ `str` 落栈构建。本函数线性扫
/// **32 位字流**（.text 按 4 字节对齐字），收集 MOVZ/MOVK 的 imm16
/// 半字按小端字节序拼流——敏感名以两个半字一段出现。
///
/// 编码（AArch64 手册位域）：`sf opc 100101 hw imm16 Rd`，opc=10 (MOVZ) /
/// 11 (MOVK)；32 位形态 sf=0（0x5280/0x7280 基），64 位 sf=1
///（0xD280/0xF280 基）。解码 = `(word >> 23) & 0x1FF` 命中四个基之一。
///
/// 诚实边界（v1）：不追踪 Rd（不同寄存器的半字也拼入同一条流——顺序
/// 仍按地址序，编译器顺序构建时正确）；不处理 lsl #48+（hw≥3 需 64 位
/// 寄存器，串构建罕见）；不认 NUL 终止（搜索按名长度滑窗）。
pub fn stack_string_hits_arm64(slices: &[&[u8]], vocab: &[&'static str]) -> Vec<&'static str> {
    let stream = arm64_immediate_stream(slices);
    search_stream(&stream, vocab)
}

/// 敏感名子串搜索（流 + 词汇表 → 去重命中）。
fn search_stream(stream: &[u8], vocab: &[&'static str]) -> Vec<&'static str> {
    let mut found = std::collections::BTreeSet::new();
    for &name in vocab {
        if name.len() < 6 {
            continue;
        }
        if stream.windows(name.len()).any(|w| w == name.as_bytes()) {
            found.insert(name);
        }
    }
    found.into_iter().collect()
}

/// AArch64 线性扫（字对齐）：MOVZ/MOVK 的 imm16 半字按小端拼流。
/// 每条指令定宽 4 字节——ARM64 无变长指令，切片按 4 字节分块解码
///（尾部不足一指令字的余数忽略，无指令语义）。
fn arm64_immediate_stream(slices: &[&[u8]]) -> Vec<u8> {
    let mut halves: Vec<u8> = Vec::new();
    for slice in slices {
        let (words, _rest) = slice.as_chunks::<4>();
        for chunk in words {
            let word = u32::from_le_bytes(*chunk);
            // MOV 宽立即数：bits[31] sf、bits[30:29] opc、bits[28:23] = 100101
            let v = (word >> 23) & 0x1FF;
            // MOVZ W32/X64（0xA5/0x1A5）与 MOVK W32/X64（0xE5/0x1E5）
            let is_movz = v == 0xA5 || v == 0x1A5;
            let is_movk = v == 0xE5 || v == 0x1E5;
            if is_movz || is_movk {
                let imm16 = ((word >> 5) & 0xFFFF) as u16;
                halves.extend_from_slice(&imm16.to_le_bytes());
            }
        }
    }
    halves
}

#[cfg(test)]
mod tests {
    use super::{stack_string_hits, stack_string_hits_arm64};

    const VOCAB: &[&str] = &["VirtualAllocEx", "WriteProcessMemory"];

    #[test]
    fn finds_name_across_consecutive_c7_immediates() {
        // mov dword [rsp+4], 'Virt'；[rsp+8], 'ualA'；[rsp+12], 'lloc'；
        // [rsp+16], 'Ex\0\0'——四段 imm32 拼出完整 "VirtualAllocEx"
        let mut code = vec![0xCCu8; 8]; // 前导填充
        code.extend_from_slice(&[
            0xC7, 0x44, 0x24, 0x04, b'V', b'i', b'r', b't', // C7 /0 mod1 SIB disp8 imm32
            0xC7, 0x44, 0x24, 0x08, b'u', b'a', b'l', b'A', 0xC7, 0x44, 0x24, 0x0C, b'l', b'l',
            b'o', b'c', 0xC7, 0x44, 0x24, 0x10, b'E', b'x', 0, 0, 0xC3,
        ]);
        let found = stack_string_hits(&[&code], VOCAB);
        assert_eq!(found, vec!["VirtualAllocEx"], "跨立即数拼接命中");
    }

    #[test]
    fn rex_w_imm64_stream_carries_half_names() {
        // 48 B8/B9 imm64（mov r64, imm64）分段携 'WritePro' + 'cessMemory'；
        // 中间穿插 nop/ret（线性扫按指令长步进，不吞它们的字节）
        let mut code = vec![0x90u8; 4];
        code.extend_from_slice(&[0x48, 0xB8]);
        code.extend_from_slice(b"WritePro");
        code.extend_from_slice(&[0x90, 0xC3]);
        code.extend_from_slice(&[0x48, 0xB9]);
        code.extend_from_slice(b"cessMemory\0\0");
        let found = stack_string_hits(&[&code], VOCAB);
        assert!(found.is_empty(), "前半段（9 字节）不构成完整名");
        code.extend_from_slice(&[0x48, 0xBA]);
        code.extend_from_slice(b"ry\0\0\0\0\0\0");
        let found = stack_string_hits(&[&code], VOCAB);
        assert_eq!(found, vec!["WriteProcessMemory"], "三段 imm64 拼接命中");
    }
    #[test]
    fn arm64_movz_movk_halves_spell_sensitive_name() {
        // "ptrace" = 'p','t','r','a','c','e' → 3 个 16 位半字（LE）：
        // 0x7470 ('p','t'), 0x6163 ('r','a'), 0x0065 ('e',0)
        let w = |imm16: u16, rd: u32| {
            // MOVZ Wd, #imm16（32 位形态）：sf=0 opc=10
            0x5280_0000 | ((imm16 as u32) << 5) | rd
        };
        let k = |imm16: u16, hw: u32, rd: u32| {
            // MOVK Wd, #imm16, lsl #hw*16
            0x7280_0000 | (hw << 21) | ((imm16 as u32) << 5) | rd
        };
        let code: Vec<u32> = vec![
            w(0x7470, 8),    // 'p','t'
            k(0x6172, 1, 8), // 'r','a'
            k(0x6563, 2, 8), // 'c','e'
            0xD65F_03C0,     // RET（打断，不影响已拼流）
        ];
        let bytes: Vec<u8> = code.iter().flat_map(|w| w.to_le_bytes()).collect();
        let found = stack_string_hits_arm64(&[&bytes], &["ptrace"]);
        assert_eq!(found, vec!["ptrace"], "三半字拼接命中");
        // 非 MOVZ/MOVK 流（普通 NOP 字）不产出
        let nops = 0xD503_201Fu32.to_le_bytes().to_vec();
        assert!(stack_string_hits_arm64(&[&nops], &["ptrace"]).is_empty());
    }
}
