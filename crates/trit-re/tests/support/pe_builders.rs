//! 带 COFF 符号表的 PE32 夹具。
//!
//! 独立成文件是为了守住 800 行预算：`support/mod.rs` 加上这只夹具后到 802 行。
//! 调用路径不变——`support::pe32_with_symbols()` 由 `mod.rs` 的 `pub use` 提供，
//! 节表写入器与 put_* 仍住在 `mod.rs`（PE/ARM64 夹具共用），这里经 `super::` 取用。

use super::{put_u16, put_u32, write_section};

/// 一个 **PE32** 样本（没有 `.pdata`，i386）：`.text` + `.rdata`、一个导入
/// （KERNEL32.dll!LoadLibraryA，IAT 在 RVA 0x2040）、`.text` 里一个 `FF 15`
/// 调用点，以及 **COFF 符号表**（2 个函数符号 + 1 个非函数符号 + 串表）。
/// PE32 共现可判的边界源就来自这张表。
pub fn pe32_with_symbols() -> Vec<u8> {
    let mut file = vec![0u8; 0x1100];

    // DOS 头
    file[..2].copy_from_slice(b"MZ");
    put_u32(&mut file, 0x3c, 0x80);

    // PE 签名 + COFF（PointerToSymbolTable=0x1000、NumberOfSymbols=3）
    file[0x80..0x84].copy_from_slice(b"PE\0\0");
    let coff = 0x84;
    put_u16(&mut file, coff, 0x014c); // i386
    put_u16(&mut file, coff + 2, 2);
    put_u32(&mut file, coff + 4, 0x6000_0000);
    put_u32(&mut file, coff + 8, 0x1000);
    put_u32(&mut file, coff + 12, 3);
    put_u16(&mut file, coff + 16, 0xE0);
    put_u16(&mut file, coff + 18, 0x0102);

    // 可选头（PE32）
    let opt = coff + 20;
    put_u16(&mut file, opt, 0x10b);
    put_u32(&mut file, opt + 16, 0x1000); // entry
    put_u32(&mut file, opt + 28, 0x400000); // image base
    put_u32(&mut file, opt + 32, 0x1000); // section align
    put_u32(&mut file, opt + 36, 0x200); // file align
    put_u32(&mut file, opt + 60, 0x400); // SizeOfHeaders
    put_u16(&mut file, opt + 68, 3); // console
    put_u32(&mut file, opt + 92, 2); // NumberOfRvaAndSizes
    let dirs = opt + 96;
    put_u32(&mut file, dirs + 8, 0x2000); // import 目录
    put_u32(&mut file, dirs + 12, 40);

    // 节表（opt + 0xE0 = 0x178）
    let sections = opt + 0xE0;
    write_section(
        &mut file,
        sections,
        b".text\0\0\0",
        0x200,
        0x1000,
        0x200,
        0x400,
        0x6000_0020,
    );
    write_section(
        &mut file,
        sections + 40,
        b".rdata\0\0",
        0x200,
        0x2000,
        0x200,
        0x600,
        0x4000_0040,
    );

    // .text @RVA 0x1010（文件 0x410）：FF 15 [绝对 IAT 0x402040]
    file[0x410] = 0xFF;
    file[0x411] = 0x15;
    put_u32(&mut file, 0x412, 0x402040);

    // 导入表 @RVA 0x2000（文件 0x600）：OriginalFirstThunk/Name/FirstThunk
    put_u32(&mut file, 0x600, 0x2030);
    put_u32(&mut file, 0x60C, 0x2050);
    put_u32(&mut file, 0x610, 0x2040);
    // IAT @0x2040（文件 0x640）：PE32 thunk 是 u32，指向 hint/name 0x2030，末尾 0
    put_u32(&mut file, 0x640, 0x2030);
    put_u32(&mut file, 0x644, 0);
    // hint/name @0x2030（文件 0x630）
    put_u16(&mut file, 0x630, 0);
    file[0x632..0x632 + 13].copy_from_slice(b"LoadLibraryA\0");
    // DLL 名 @0x2050（文件 0x650）
    file[0x650..0x65D].copy_from_slice(b"KERNEL32.dll\0");

    // COFF 符号表 @0x1000：3 条 × 18 字节（Name(8) Value(+8) Section(+12)
    // Type(+14) Storage(+16) Aux(+17)），串表 @0x1036。
    let mut sym = |base: usize, name: [u8; 8], value: u32, section: u16, ctype: u16| {
        file[base..base + 8].copy_from_slice(&name);
        put_u32(&mut file, base + 8, value);
        put_u16(&mut file, base + 12, section);
        put_u16(&mut file, base + 14, ctype);
        file[base + 16] = 2; // IMAGE_SYM_CLASS_EXTERNAL
        file[base + 17] = 0; // aux
    };
    // sym0：短名内联 "fn_alpha"（正好 8 字节无 NUL），RVA 0x1010，节 1，函数
    sym(0x1000, *b"fn_alpha", 0x1010, 1, 0x0200);
    // sym1：长名（串表偏移 4），RVA 0x1100，节 1，函数
    sym(0x1012, [0, 0, 0, 0, 4, 0, 0, 0], 0x1100, 1, 0x0200);
    // sym2：非函数符号（Type 高字节 = 0），但它住在**可执行节** .text 里 ⇒ 只有 Type
    // 那道过滤能挡下它。放在 .rdata 的话「节必须可执行」会替 Type 把关，符号类型
    // 过滤就成了空转代码（变异门正是这样抓到它的）。
    sym(0x1024, *b"adata\0\0\0", 0x1180, 1, 0x0000);

    // 串表 @0x1036（3×18=54）：u32 大小 + 长名
    let long_name = b"fn_beta_long_name\0";
    put_u32(&mut file, 0x1036, (4 + long_name.len()) as u32);
    file[0x103A..0x103A + long_name.len()].copy_from_slice(long_name);

    file
}
