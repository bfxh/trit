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
