//! 合成夹具：手工拼最小但结构完整的 PE32+ / ELF64。
//!
//! 不依赖任何样本文件，偏移量在注释里写死，方便逐字段对账。
//! 每个测试目标只用到一部分构造函数，故整模块放行 dead_code。

#![allow(dead_code)]

pub fn put_u16(buf: &mut [u8], off: usize, value: u16) {
    buf[off..off + 2].copy_from_slice(&value.to_le_bytes());
}

pub fn put_u32(buf: &mut [u8], off: usize, value: u32) {
    buf[off..off + 4].copy_from_slice(&value.to_le_bytes());
}

pub fn put_u64(buf: &mut [u8], off: usize, value: u64) {
    buf[off..off + 8].copy_from_slice(&value.to_le_bytes());
}

fn fill_noise(buf: &mut [u8], mut state: u32) {
    for byte in buf.iter_mut() {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        *byte = (state >> 7) as u8;
    }
}

const CERT_OID_SIGNED_DATA: &[u8] = &[
    0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x07, 0x02,
];

/// 一个 PE32+ 样本：2 节、1 个导入、1 个导出、PDB、安全目录、8 KiB 高熵叠加段。
pub fn pe_sample() -> Vec<u8> {
    let overlay_len = 8192usize;
    // .text 0x200..0x600；.rdata 0x600..0xA00；.pdata 0xA00..0xC00；叠加段其后。
    let mut file = vec![0u8; 0xC00 + overlay_len];

    // DOS 头 + stub
    file[..2].copy_from_slice(b"MZ");
    put_u32(&mut file, 0x3c, 0x80);
    file[0x50..0x54].copy_from_slice(b"Rich");

    // PE 签名 + COFF
    file[0x80..0x84].copy_from_slice(b"PE\0\0");
    let coff = 0x84;
    put_u16(&mut file, coff, 0x8664);
    put_u16(&mut file, coff + 2, 3);
    put_u32(&mut file, coff + 4, 0x6000_0000);
    put_u16(&mut file, coff + 16, 0xF0);
    put_u16(&mut file, coff + 18, 0x22);

    // 可选头（PE32+）
    let opt = coff + 20;
    put_u16(&mut file, opt, 0x20b);
    put_u32(&mut file, opt + 16, 0x1000);
    put_u64(&mut file, opt + 24, 0x1_4000_0000);
    put_u32(&mut file, opt + 32, 0x1000);
    put_u32(&mut file, opt + 36, 0x200);
    put_u32(&mut file, opt + 56, 0x3000);
    put_u32(&mut file, opt + 60, 0x200);
    put_u16(&mut file, opt + 68, 3);
    put_u32(&mut file, opt + 108, 16);
    let dirs = opt + 112;
    put_u32(&mut file, dirs, 0x20A0); // export
    put_u32(&mut file, dirs + 4, 40);
    put_u32(&mut file, dirs + 8, 0x2000); // import
    put_u32(&mut file, dirs + 12, 40);
    put_u32(&mut file, dirs + 24, 0x3000); // exception (.pdata)
    put_u32(&mut file, dirs + 28, 12);
    put_u32(&mut file, dirs + 32, 0x7A0); // security（文件偏移）
    put_u32(&mut file, dirs + 36, 0x60);
    put_u32(&mut file, dirs + 48, 0x2100); // debug
    put_u32(&mut file, dirs + 52, 28);
    put_u32(&mut file, dirs + 72, 0x2300); // TLS
    put_u32(&mut file, dirs + 76, 40);
    put_u32(&mut file, dirs + 112, 0x2380); // CLR
    put_u32(&mut file, dirs + 116, 72);

    // 节表
    let sections = opt + 0xF0;
    write_section(
        &mut file,
        sections,
        b".text\0\0\0",
        0x400,
        0x1000,
        0x400,
        0x200,
        0x6000_0020,
    );
    write_section(
        &mut file,
        sections + 40,
        b".rdata\0\0",
        0x400,
        0x2000,
        0x400,
        0x600,
        0x4000_0040,
    );
    write_section(
        &mut file,
        sections + 80,
        b".pdata\0\0",
        0x18,
        0x3000,
        0x200,
        0xA00,
        0x4000_0040,
    );
    // .pdata：一个函数 0x1000..0x1030，包含两个调用点。
    put_u32(&mut file, 0xA00, 0x1000);
    put_u32(&mut file, 0xA04, 0x1030);
    put_u32(&mut file, 0xA08, 0);

    // .text 偏移 0x10（RVA 0x1010）：FF 15 [IAT 0x2040]，下一条 RVA 0x1016
    file[0x210] = 0xFF;
    file[0x211] = 0x15;
    file[0x212..0x216].copy_from_slice(&0x102Au32.to_le_bytes());
    // 偏移 0x20（RVA 0x1020）：FF 15 [IAT 0x2048]，下一条 RVA 0x1026
    file[0x220] = 0xFF;
    file[0x221] = 0x15;
    file[0x222..0x226].copy_from_slice(&0x1022u32.to_le_bytes());

    // 导入表：KERNEL32.dll!LoadLibraryA
    put_u32(&mut file, 0x600, 0x2040);
    put_u32(&mut file, 0x60C, 0x2080);
    put_u32(&mut file, 0x610, 0x2040);
    put_u64(&mut file, 0x640, 0x2060);
    put_u64(&mut file, 0x648, 0x2070);
    put_u16(&mut file, 0x660, 0);
    let load_library = b"LoadLibraryA\0";
    file[0x662..0x662 + load_library.len()].copy_from_slice(load_library);
    // 第二个导入：hint/name 在 0x670（RVA 0x2070），槽位 0x2048。
    put_u16(&mut file, 0x670, 0);
    let connect = b"connect\0";
    file[0x672..0x672 + connect.len()].copy_from_slice(connect);
    file[0x680..0x68D].copy_from_slice(b"KERNEL32.dll\0");

    // 导出表：DemoExport
    put_u32(&mut file, 0x6AC, 0x2080);
    put_u32(&mut file, 0x6B0, 1);
    put_u32(&mut file, 0x6B4, 1);
    put_u32(&mut file, 0x6B8, 1);
    put_u32(&mut file, 0x6BC, 0x2120);
    put_u32(&mut file, 0x6C0, 0x20C8);
    put_u32(&mut file, 0x6C4, 0x2140);
    put_u32(&mut file, 0x6C8, 0x20D0);
    let export_name = b"DemoExport\0";
    file[0x6D0..0x6D0 + export_name.len()].copy_from_slice(export_name);

    // 调试目录 + RSDS + PDB
    put_u32(&mut file, 0x70C, 2);
    put_u32(&mut file, 0x710, 0x40);
    put_u32(&mut file, 0x714, 0x2120);
    put_u32(&mut file, 0x718, 0x760);
    file[0x760..0x764].copy_from_slice(b"RSDS");
    put_u32(&mut file, 0x774, 1);
    let pdb = b"D:\\build\\demo.pdb\0";
    file[0x778..0x778 + pdb.len()].copy_from_slice(pdb);

    // 安全目录：WIN_CERTIFICATE + signedData OID
    put_u32(&mut file, 0x7A0, 0x60);
    put_u16(&mut file, 0x7A4, 0x0200);
    put_u16(&mut file, 0x7A6, 0x0002);
    file[0x7A8..0x7A8 + CERT_OID_SIGNED_DATA.len()].copy_from_slice(CERT_OID_SIGNED_DATA);

    // TLS 目录（RVA 0x2300）：AddressOfCallBacks = base + 0x2340
    put_u64(&mut file, 0x900 + 24, 0x1_4000_2340);
    // 回调数组（RVA 0x2340）：两个回调指向 .text 内
    put_u64(&mut file, 0x940, 0x1_4000_1010);
    put_u64(&mut file, 0x948, 0x1_4000_1020);
    // CLR 头（RVA 0x2380）
    put_u32(&mut file, 0x980, 72);
    put_u16(&mut file, 0x984, 2);
    put_u16(&mut file, 0x986, 5);
    put_u32(&mut file, 0x988, 0x23C0); // metadata RVA
    put_u32(&mut file, 0x98C, 0x40);
    put_u32(&mut file, 0x990, 1); // ILONLY
    put_u32(&mut file, 0x994, 0x0600_0001);
    // 元数据根（RVA 0x23C0）：BSJB + 版本串
    put_u32(&mut file, 0x9C0, 0x424A_5342);
    put_u16(&mut file, 0x9C4, 1);
    put_u16(&mut file, 0x9C6, 1);
    put_u32(&mut file, 0x9CC, 16);
    let version = b"v4.0.30319\0";
    file[0x9D0..0x9D0 + version.len()].copy_from_slice(version);

    fill_noise(&mut file[0xC00..], 0x1234_5678);
    file
}

#[allow(clippy::too_many_arguments)]
fn write_section(
    file: &mut [u8],
    base: usize,
    name: &[u8; 8],
    virtual_size: u32,
    virtual_address: u32,
    raw_size: u32,
    raw_ptr: u32,
    characteristics: u32,
) {
    file[base..base + 8].copy_from_slice(name);
    put_u32(file, base + 8, virtual_size);
    put_u32(file, base + 12, virtual_address);
    put_u32(file, base + 16, raw_size);
    put_u32(file, base + 20, raw_ptr);
    put_u32(file, base + 36, characteristics);
}

const SHSTRTAB: &[u8] =
    b"\0.text\0.dynstr\0.dynsym\0.comment\0.note.gnu.build-id\0.plt\0.rela.plt\0.shstrtab\0";

/// 一个 ELF64 样本：7 节、`socket` 动态导入、GCC 指纹、GNU build-id。
///
/// `packed` 为真时追加 8 KiB 高熵叠加段；`stripped` 为真时删掉节表。
pub fn elf_sample(packed: bool, stripped: bool) -> Vec<u8> {
    let overlay_len = if packed { 8192 } else { 512 };
    let mut file = vec![0u8; 0x740 + overlay_len];

    file[..4].copy_from_slice(b"\x7fELF");
    file[4] = 2; // ELF64
    file[5] = 1; // 小端
    file[6] = 1;
    put_u16(&mut file, 16, 2); // ET_EXEC
    put_u16(&mut file, 18, 62);
    put_u32(&mut file, 20, 1);
    put_u64(&mut file, 24, 0x40_0100);
    put_u64(&mut file, 32, 0x40);
    put_u64(&mut file, 40, if stripped { 0 } else { 0x500 });
    put_u16(&mut file, 52, 64);
    put_u16(&mut file, 54, 56);
    put_u16(&mut file, 56, 1);
    put_u16(&mut file, 58, 64);
    put_u16(&mut file, 60, if stripped { 0 } else { 9 });
    put_u16(&mut file, 62, if stripped { 0 } else { 8 });

    // PT_LOAD：可读可执行，覆盖 0x400000..0x400400
    put_u32(&mut file, 0x40, 1);
    put_u32(&mut file, 0x44, 5);
    put_u64(&mut file, 0x48, 0);
    put_u64(&mut file, 0x50, 0x40_0000);
    // 文件内只覆盖到 0x400（.text），内存里覆盖到 0x1000（含 .plt）。
    put_u64(&mut file, 0x60, 0x400);
    put_u64(&mut file, 0x68, 0x1000);

    if packed {
        fill_noise(&mut file[0x100..0x300], 0x9E37_79B9);
    }
    // .text 偏移 0x10（VA 0x400110）：E8 rel32 → PLT 桩 0x400500
    file[0x110] = 0xE8;
    file[0x111..0x115].copy_from_slice(&0x3EBu32.to_le_bytes());

    // .dynstr："\0socket\0"
    file[0x300] = 0;
    file[0x301..0x308].copy_from_slice(b"socket\0");
    // .dynsym：0 号空，1 号未定义符号 socket
    put_u32(&mut file, 0x398, 1);
    file[0x39C] = 0x12;
    put_u16(&mut file, 0x39E, 0);
    // .comment
    let comment = b"GCC: (GNU) 13.2.0\0";
    file[0x3B0..0x3B0 + comment.len()].copy_from_slice(comment);
    // .note.gnu.build-id
    put_u32(&mut file, 0x400, 4);
    put_u32(&mut file, 0x404, 8);
    put_u32(&mut file, 0x408, 3);
    file[0x40C..0x410].copy_from_slice(b"GNU\0");
    file[0x410..0x418].copy_from_slice(&[0xde, 0xad, 0xbe, 0xef, 0x01, 0x02, 0x03, 0x04]);
    // .plt：FF 25 [GOT 0x400600]，下一条 VA 0x400506
    file[0x4A0] = 0xFF;
    file[0x4A1] = 0x25;
    file[0x4A2..0x4A6].copy_from_slice(&0xFAu32.to_le_bytes());
    // .rela.plt：R_X86_64_JUMP_SLOT，GOT 0x400600 → 符号 1（socket）
    file[0x4C0..0x4C8].copy_from_slice(&0x40_0600u64.to_le_bytes());
    file[0x4C8..0x4D0].copy_from_slice(&((1u64 << 32) | 7).to_le_bytes());
    file[0x4D0..0x4D8].copy_from_slice(&0i64.to_le_bytes());
    // .shstrtab
    file[0x440..0x440 + SHSTRTAB.len()].copy_from_slice(SHSTRTAB);

    if !stripped {
        // 0 号节必须是全零的 NULL 节，真实节从 +64 开始。
        let sh = 0x500;
        write_shdr(&mut file, sh + 64, 1, 1, 6, 0x40_0100, 0x100, 0x200, 0, 0);
        write_shdr(&mut file, sh + 128, 7, 3, 2, 0x40_0300, 0x300, 0x80, 0, 0);
        write_shdr(
            &mut file,
            sh + 192,
            15,
            11,
            2,
            0x40_0380,
            0x380,
            0x30,
            2,
            24,
        );
        write_shdr(&mut file, sh + 256, 23, 1, 0, 0, 0x3B0, 0x20, 0, 0);
        write_shdr(&mut file, sh + 320, 32, 7, 2, 0x40_0400, 0x400, 0x24, 0, 0);
        write_shdr(&mut file, sh + 384, 51, 1, 6, 0x40_0500, 0x4A0, 0x20, 0, 16);
        write_shdr(&mut file, sh + 448, 56, 4, 2, 0x40_04C0, 0x4C0, 0x18, 3, 24);
        write_shdr(&mut file, sh + 512, 67, 3, 0, 0, 0x440, 0x60, 0, 0);
    }

    if packed {
        fill_noise(&mut file[0x740..], 0x0BAD_F00D);
    }
    file
}

#[allow(clippy::too_many_arguments)]
fn write_shdr(
    file: &mut [u8],
    base: usize,
    name: u32,
    sh_type: u32,
    flags: u64,
    addr: u64,
    offset: u64,
    size: u64,
    link: u32,
    entsize: u64,
) {
    put_u32(file, base, name);
    put_u32(file, base + 4, sh_type);
    put_u64(file, base + 8, flags);
    put_u64(file, base + 16, addr);
    put_u64(file, base + 24, offset);
    put_u64(file, base + 32, size);
    put_u32(file, base + 40, link);
    put_u32(file, base + 44, 0);
    put_u64(file, base + 48, 1);
    put_u64(file, base + 56, entsize);
}

// ---------- ELF32（i386）夹具 ----------

/// 最小 ELF32：3 个导入，覆盖 JUMP_SLOT / GLOB_DAT、PIC 桩、基址解析与"只取地址"。
///
/// - `socket`（JUMP_SLOT）：`E8` 打到 PLT 桩、`call *0x8(%ebx)`、`jmp *abs32` 三个引用；
/// - `printf`（GLOB_DAT，`.rel.dyn`）：有 PLT 桩、被 `E8` 调用 ⇒ 必须判 `True`
///   （只收 JUMP_SLOT 的旧实现会把它当成"没有槽位"）；
/// - `getenv`（JUMP_SLOT）：只有一次 `push $getenv@plt`（立即数取址）⇒ 降级 `Unknown`；
/// - `memcmp`（JUMP_SLOT）：只有一次 `mov ecx,[eax+0x1C]`——基址在 **EAX**（`get_pc_thunk.ax`）⇒ 降级 `Unknown`。
///
/// `thunk = false` 时去掉 `call get_pc_thunk.bx; add $imm,%ebx`：`(%ebx)` 引用解析不了，
/// 报告必须标不完整，而不是判"没有调用点"。
pub fn elf32_sample(thunk: bool) -> Vec<u8> {
    const TEXT: u32 = 0x0804_8100;
    const PLT: u32 = 0x0804_8140;
    const GOT: u32 = 0x0804_9000;
    let mut file = vec![0u8; 0x560];

    file[..4].copy_from_slice(b"ELF");
    file[4] = 1; // ELF32
    file[5] = 1; // 小端
    file[6] = 1;
    put_u16(&mut file, 16, 2); // ET_EXEC
    put_u16(&mut file, 18, 3); // EM_386
    put_u32(&mut file, 20, 1);
    put_u32(&mut file, 24, TEXT);
    put_u32(&mut file, 28, 0); // phoff
    put_u32(&mut file, 32, 0x400); // shoff
    put_u16(&mut file, 40, 52);
    put_u16(&mut file, 42, 32);
    put_u16(&mut file, 44, 0); // phnum
    put_u16(&mut file, 46, 40);
    put_u16(&mut file, 48, 8);
    put_u16(&mut file, 50, 7);

    // .text：0x100 → VA 0x08048100
    if thunk {
        // 0x100: call 0x128（0x128 - 0x105 = 0x23）
        file[0x100] = 0xE8;
        put_u32(&mut file, 0x101, 0x23);
        // 0x105: add $imm,%ebx ⇒ ebx = 0x08048105 + imm = GOT
        file[0x105] = 0x81;
        file[0x106] = 0xC3;
        put_u32(&mut file, 0x107, GOT.wrapping_sub(TEXT + 5));
    }
    // 0x10B: call 0x140（socket 的 PLT 桩；0x140 - 0x110 = 0x30）
    file[0x10B] = 0xE8;
    put_u32(&mut file, 0x10C, 0x30);
    // 0x110: call *0x8(%ebx)（socket 槽位 = GOT + 8）
    file[0x110] = 0xFF;
    file[0x111] = 0x93;
    put_u32(&mut file, 0x112, 8);
    // 0x116: push $0x08048160（getenv 的 PLT 桩地址——非 PIC 下取函数地址即取桩地址）
    file[0x116] = 0x68;
    put_u32(&mut file, 0x117, PLT + 0x20);
    // 0x11C: jmp *abs32（非 PIC 风格，直接给出 socket 槽位）
    file[0x11C] = 0xFF;
    file[0x11D] = 0x25;
    put_u32(&mut file, 0x11E, GOT + 8);
    // 0x122: call 0x150（printf 的 PLT 桩；0x150 - 0x127 = 0x29）
    file[0x122] = 0xE8;
    put_u32(&mut file, 0x123, 0x29);
    // 0x128: __x86.get_pc_thunk.bx
    file[0x128..0x12C].copy_from_slice(&[0x8B, 0x1C, 0x24, 0xC3]);
    if thunk {
        // 0x12C: call 0x13C（get_pc_thunk.ax；0x13C - 0x131 = 0x0B）
        file[0x12C] = 0xE8;
        put_u32(&mut file, 0x12D, 0x0B);
        // 0x131: add $imm,%eax ⇒ eax = 0x08048131 + imm = 0x08048FF8
        file[0x131] = 0x05;
        put_u32(&mut file, 0x132, 0x0804_8FF8u32.wrapping_sub(TEXT + 0x31));
        // 0x136: mov ecx,[eax+0x1C]（memcmp 槽位 = 0x08048FF8 + 0x1C = GOT + 0x14）
        file[0x136] = 0x8B;
        file[0x137] = 0x88;
        put_u32(&mut file, 0x138, 0x1C);
        // 0x13C: __x86.get_pc_thunk.ax
        file[0x13C..0x140].copy_from_slice(&[0x8B, 0x04, 0x24, 0xC3]);
    }

    // .plt：0x140 → VA 0x08048140；两个桩（socket 走 JUMP_SLOT，printf 走 GLOB_DAT）
    file[0x140] = 0xFF;
    file[0x141] = 0xA3;
    put_u32(&mut file, 0x142, 8);
    file[0x150] = 0xFF;
    file[0x151] = 0xA3;
    put_u32(&mut file, 0x152, 0xC);
    file[0x160] = 0xFF;
    file[0x161] = 0xA3;
    put_u32(&mut file, 0x162, 0x10);

    // .dynstr："\0socket\0printf\0getenv\0"
    file[0x180] = 0;
    file[0x181..0x188].copy_from_slice(b"socket\0");
    file[0x188..0x18F].copy_from_slice(b"printf\0");
    file[0x18F..0x196].copy_from_slice(b"getenv\0");
    file[0x196..0x19D].copy_from_slice(b"memcmp\0");
    // .dynsym：0 号空，1 socket、2 printf、3 getenv（STT_FUNC | STB_GLOBAL，未定义）
    put_u32(&mut file, 0x1B0, 1);
    file[0x1BC] = 0x12;
    put_u32(&mut file, 0x1C0, 8);
    file[0x1CC] = 0x12;
    put_u32(&mut file, 0x1D0, 15);
    file[0x1DC] = 0x12;
    put_u32(&mut file, 0x1E0, 22);
    file[0x1EC] = 0x12;

    // .rel.plt：3 条 JUMP_SLOT（类型 7）；.rel.dyn：1 条 GLOB_DAT（类型 6）
    put_u32(&mut file, 0x1F0, GOT + 8);
    put_u32(&mut file, 0x1F4, (1 << 8) | 7);
    put_u32(&mut file, 0x1F8, GOT + 0x10);
    put_u32(&mut file, 0x1FC, (3 << 8) | 7);
    put_u32(&mut file, 0x200, GOT + 0x14);
    put_u32(&mut file, 0x204, (4 << 8) | 7);
    put_u32(&mut file, 0x208, GOT + 0xC);
    put_u32(&mut file, 0x20C, (2 << 8) | 6);

    // .shstrtab
    let shstrtab = b"\0.text\0.plt\0.dynstr\0.dynsym\0.rel.plt\0.rel.dyn\0.shstrtab\0";
    file[0x220..0x220 + shstrtab.len()].copy_from_slice(shstrtab);

    let sh = 0x400;
    write_shdr32(&mut file, sh + 40, 1, 1, 6, TEXT, 0x100, 0x40, 0, 0);
    write_shdr32(&mut file, sh + 80, 7, 1, 6, PLT, 0x140, 0x30, 0, 0);
    write_shdr32(&mut file, sh + 120, 12, 3, 2, 0, 0x180, 0x1D, 0, 0);
    write_shdr32(&mut file, sh + 160, 20, 11, 2, 0, 0x1A0, 0x50, 3, 16);
    write_shdr32(&mut file, sh + 200, 28, 9, 2, 0, 0x1F0, 0x18, 4, 8);
    write_shdr32(&mut file, sh + 240, 37, 9, 2, 0, 0x208, 0x08, 4, 8);
    write_shdr32(
        &mut file,
        sh + 280,
        46,
        3,
        0,
        0,
        0x220,
        shstrtab.len() as u32,
        0,
        0,
    );
    file
}

#[allow(clippy::too_many_arguments)]
fn write_shdr32(
    file: &mut [u8],
    base: usize,
    name: u32,
    sh_type: u32,
    flags: u32,
    addr: u32,
    offset: u32,
    size: u32,
    link: u32,
    entsize: u32,
) {
    put_u32(file, base, name);
    put_u32(file, base + 4, sh_type);
    put_u32(file, base + 8, flags);
    put_u32(file, base + 12, addr);
    put_u32(file, base + 16, offset);
    put_u32(file, base + 20, size);
    put_u32(file, base + 24, link);
    put_u32(file, base + 28, 0);
    put_u32(file, base + 32, 4);
    put_u32(file, base + 36, entsize);
}

// ---------- ARM64（AArch64）夹具 ----------

/// `adrp xR, page_of(target)`。
pub fn arm64_adrp(pc: u64, target: u64, register: u32) -> u32 {
    let delta = ((target & !0xFFF) as i64 - (pc & !0xFFF) as i64) >> 12;
    let imm = delta as u32 & 0x1F_FFFF;
    0x9000_0000 | ((imm & 0x3) << 29) | (((imm >> 2) & 0x7_FFFF) << 5) | register
}

/// `ldr xT, [xR, #offset]`（64 位无符号偏移，offset 必须是 8 的倍数）。
pub fn arm64_ldr(register: u32, offset: u64, target: u32) -> u32 {
    assert_eq!(offset % 8, 0);
    0xF940_0000 | (((offset / 8) as u32) << 10) | (register << 5) | target
}

/// `blr xR`。
pub fn arm64_blr(register: u32) -> u32 {
    0xD63F_0000 | (register << 5)
}

/// `br xR`。
pub fn arm64_br(register: u32) -> u32 {
    0xD61F_0000 | (register << 5)
}

/// `bl target`。
pub fn arm64_bl(pc: u64, target: u64) -> u32 {
    let delta = ((target as i64 - pc as i64) >> 2) as u32 & 0x03FF_FFFF;
    0x9400_0000 | delta
}

pub fn put_insn(buf: &mut [u8], off: usize, insn: u32) {
    buf[off..off + 4].copy_from_slice(&insn.to_le_bytes());
}

/// 最小 PE32+ ARM64：1 个导入、`.pdata` 8 字节条目（0x1000 起）。
///
/// `thunk` 为真时 `.text` 先 `bl` 到 0x1020 的导入跳板；否则直接在 0x1008 `blr`。
pub fn pe_arm64_sample(thunk: bool) -> Vec<u8> {
    let mut file = vec![0u8; 0xC00];
    file[..2].copy_from_slice(b"MZ");
    put_u32(&mut file, 0x3c, 0x40);
    file[0x40..0x44].copy_from_slice(b"PE\0\0");
    let coff = 0x44;
    put_u16(&mut file, coff, 0xaa64);
    put_u16(&mut file, coff + 2, 3);
    put_u16(&mut file, coff + 16, 0xF0);
    let opt = coff + 20;
    put_u16(&mut file, opt, 0x20b);
    put_u32(&mut file, opt + 16, 0x1000);
    put_u64(&mut file, opt + 24, 0x1_4000_0000);
    put_u32(&mut file, opt + 32, 0x1000);
    put_u32(&mut file, opt + 36, 0x200);
    put_u32(&mut file, opt + 56, 0x4000);
    put_u32(&mut file, opt + 60, 0x200);
    put_u16(&mut file, opt + 68, 3);
    put_u32(&mut file, opt + 108, 16);
    let dirs = opt + 112;
    put_u32(&mut file, dirs + 8, 0x2000);
    put_u32(&mut file, dirs + 12, 40);
    put_u32(&mut file, dirs + 24, 0x3000);
    put_u32(&mut file, dirs + 28, 8);

    let sections = opt + 0xF0;
    write_section(
        &mut file,
        sections,
        b".text\0\0\0",
        0x400,
        0x1000,
        0x400,
        0x200,
        0x6000_0020,
    );
    write_section(
        &mut file,
        sections + 40,
        b".rdata\0\0",
        0x400,
        0x2000,
        0x400,
        0x600,
        0x4000_0040,
    );
    write_section(
        &mut file,
        sections + 80,
        b".pdata\0\0",
        0x8,
        0x3000,
        0x200,
        0xA00,
        0x4000_0040,
    );
    // ARM64 .pdata 条目：begin + unwind（8 字节），结束地址由下一条推。
    put_u32(&mut file, 0xA00, 0x1000);
    put_u32(&mut file, 0xA04, 0);

    // 导入表：KERNEL32.dll!LoadLibraryA，IAT 槽位 0x2040
    let descriptor = 0x600usize;
    put_u32(&mut file, descriptor, 0x2040);
    put_u32(&mut file, descriptor + 12, 0x2080);
    put_u32(&mut file, descriptor + 16, 0x2040);
    put_u64(&mut file, 0x640, 0x2060);
    put_u16(&mut file, 0x660, 0);
    let load_library = b"LoadLibraryA\0";
    file[0x662..0x662 + load_library.len()].copy_from_slice(load_library);
    file[0x680..0x68D].copy_from_slice(b"KERNEL32.dll\0");

    if thunk {
        // 0x1000: bl 0x1020；0x1020: 跳板 adrp/ldr/br
        put_insn(&mut file, 0x200, arm64_bl(0x1000, 0x1020));
        put_insn(&mut file, 0x220, arm64_adrp(0x1020, 0x2000, 16));
        put_insn(&mut file, 0x224, arm64_ldr(16, 0x40, 17));
        put_insn(&mut file, 0x228, arm64_br(17));
    } else {
        put_insn(&mut file, 0x200, arm64_adrp(0x1000, 0x2000, 9));
        put_insn(&mut file, 0x204, arm64_ldr(9, 0x40, 9));
        put_insn(&mut file, 0x208, arm64_blr(9));
    }
    file
}

/// 最小 ELF64 ARM64：1 个导入（`socket`）、JUMP_SLOT(1026)、PLT 桩 + `bl` 调用方。
pub fn elf_arm64_sample() -> Vec<u8> {
    let mut file = vec![0u8; 0x740 + 512];
    file[..4].copy_from_slice(b"\x7fELF");
    file[4] = 2;
    file[5] = 1;
    file[6] = 1;
    put_u16(&mut file, 16, 2);
    put_u16(&mut file, 18, 183);
    put_u32(&mut file, 20, 1);
    put_u64(&mut file, 24, 0x40_0100);
    put_u64(&mut file, 32, 0x40);
    put_u64(&mut file, 40, 0x500);
    put_u16(&mut file, 52, 64);
    put_u16(&mut file, 54, 56);
    put_u16(&mut file, 56, 1);
    put_u16(&mut file, 58, 64);
    put_u16(&mut file, 60, 9);
    put_u16(&mut file, 62, 8);
    put_u32(&mut file, 0x40, 1);
    put_u32(&mut file, 0x44, 5);
    put_u64(&mut file, 0x50, 0x40_0000);
    put_u64(&mut file, 0x60, 0x400);
    put_u64(&mut file, 0x68, 0x1000);

    // .text：0x400110 处 bl → PLT 桩 0x400500
    put_insn(&mut file, 0x110, arm64_bl(0x40_0110, 0x40_0500));

    // .dynstr: "\0socket\0"
    file[0x300] = 0;
    file[0x301..0x308].copy_from_slice(b"socket\0");
    put_u32(&mut file, 0x398, 1);
    file[0x39C] = 0x12;
    let comment = b"GCC: (GNU) 13.2.0\0";
    file[0x3B0..0x3B0 + comment.len()].copy_from_slice(comment);
    put_u32(&mut file, 0x400, 4);
    put_u32(&mut file, 0x404, 8);
    put_u32(&mut file, 0x408, 3);
    file[0x40C..0x410].copy_from_slice(b"GNU\0");
    file[0x410..0x418].copy_from_slice(&[0xde, 0xad, 0xbe, 0xef, 1, 2, 3, 4]);
    file[0x440..0x440 + SHSTRTAB.len()].copy_from_slice(SHSTRTAB);

    // .plt：adrp x16, 0x400000; ldr x17, [x16, #0x600]; br x17
    put_insn(&mut file, 0x4A0, arm64_adrp(0x40_0500, 0x40_0600, 16));
    put_insn(&mut file, 0x4A4, arm64_ldr(16, 0x600, 17));
    put_insn(&mut file, 0x4A8, arm64_br(17));
    // .rela.plt：JUMP_SLOT(1026)，GOT 0x400600 → 符号 1（socket）
    put_u64(&mut file, 0x4C0, 0x40_0600);
    put_u64(&mut file, 0x4C8, (1u64 << 32) | 1026);
    put_u64(&mut file, 0x4D0, 0);

    let sh = 0x500;
    write_shdr(&mut file, sh + 64, 1, 1, 6, 0x40_0100, 0x100, 0x200, 0, 0);
    write_shdr(&mut file, sh + 128, 7, 3, 2, 0x40_0300, 0x300, 0x80, 0, 0);
    write_shdr(
        &mut file,
        sh + 192,
        15,
        11,
        2,
        0x40_0380,
        0x380,
        0x30,
        2,
        24,
    );
    write_shdr(&mut file, sh + 256, 23, 1, 0, 0, 0x3B0, 0x20, 0, 0);
    write_shdr(&mut file, sh + 320, 32, 7, 2, 0x40_0400, 0x400, 0x24, 0, 0);
    write_shdr(&mut file, sh + 384, 51, 1, 6, 0x40_0500, 0x4A0, 0x20, 0, 16);
    write_shdr(&mut file, sh + 448, 56, 4, 2, 0x40_04C0, 0x4C0, 0x18, 3, 24);
    write_shdr(&mut file, sh + 512, 67, 3, 0, 0, 0x440, 0x60, 0, 0);
    file
}
