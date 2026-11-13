# 03 pe 对象模型：从字节到对象树

> 这份文件规定 `crates/trit-re/src/sigscan/pe2/` 里要写什么。
> 依据是 libyara v4.5.4 的 `modules/pe/pe.c` 与 `pe_utils.c`，照它的逻辑从**原始字节**求值。不要用 `crate::pe::PeInfo`：两边的解析规则不一样，基线的 `entry_point` 差异就是这么来的。
> 验证：`tests/data/yara_expr/pe_fields.tsv`（1252 条官方读数，其中 1088 条是逐字段的精确值）和 `expr_core.tsv` 的 I 组。

---

## 1. 你要交付的两个函数

接缝补丁里已经有占位（`sigscan/pe2.rs`）：

```rust
/// 从完整文件字节构造 pe 对象树。
pub(super) fn build(data: &[u8]) -> Node

/// 函数调用。owner = 函数所属的对象结点，name = 函数名，
/// args = 实参表达式（正则实参从这里取），vals = 已求值且全部有定义的实参。
pub(super) fn call(owner: &Node, name: &str, args: &[Expr], vals: &[Val]) -> Val
```

`Node` 在 `sigscan/modreg.rs`：

```rust
enum Node {
    Int(i64), Str(Vec<u8>), Undef, Unknown,
    Struct { fields: Vec<(&'static str, Node)>, missing_undef: bool },
    Array(Vec<Node>),
    Dict(Vec<(Vec<u8>, Node)>),
}
```

路径导航、下标截断、字典查找、迭代、实参的 undefined 传播，`modreg.rs` 都已经做完。你只管造树和写函数。

### 1.1 树的形状

**不是 PE**（`rva::pe_header(data)` 返回 `None`）：

```rust
Node::Struct { fields: vec![("is_pe", Node::Int(0))], missing_undef: true }
```

`missing_undef: true` 让其它所有字段都是 `Undef`，与官方一致。

**是 PE**：

```rust
Node::Struct { fields: vec![("is_pe", Node::Int(1)), ("machine", …), …], missing_undef: false }
```

`missing_undef: false` 让你**没放进树里的字段**自动成为 `Unknown`（记名弃权）。所以可以一块一块地加，没写完的部分不会给出错误答案。

嵌套结构（`linker_version`、`overlay`、`rich_signature` 等）和数组元素也是 `Node::Struct`，同样用 `missing_undef: false`。

字段的值：有值放 `Node::Int` / `Node::Str`；官方明确是 undefined 的放 `Node::Undef`；拿不准的**不放**。

### 1.2 函数怎么拿到数据

`call` 只收到 `owner` 结点，拿不到原始字节。办法：`build` 把函数需要的东西也放进树里，用下划线开头的私有字段名（规则里写不出来，因为 schema 里没有）。例如：

- `("_is_64", Node::Int(0 或 1))`
- `("_rva_table", …)`：`rva_to_offset` 需要的节表摘要，或者直接把 `data` 的引用带进来。

更简单的做法是把 `build` 和 `call` 改成一个带状态的结构体的方法，结构体里存 `data: &[u8]` 和解析好的 `PeHeader`。这需要改 `modreg.rs` 里调用它们的两处，属于允许的改动。选哪种都行，选定之后保持一致。

---

## 2. 读字节的规矩

- 所有多字节字段都是小端。
- **每一次读都要检查边界**。官方的写法是 `struct_fits_in_pe(pe, 指针, 类型)`，意思是“这个结构整个落在文件内”。对应到 Rust 就是 `data.get(off..off + size)`，拿不到就按该处写明的方式处理（多数是“停止这一项”）。
- 文件偏移用 `u64` 或 `usize` 算，加法用 `checked_add`。
- `rva::rva_to_offset(data, &header, rva)` 已经写好并按官方读数测过，返回 `None` 对应官方的 -1。

---

## 3. 第一段：头部、数据目录、节表、overlay（任务 T20）

记 `nt` = `e_lfanew`，`coff` = `nt + 4`，`opt` = `nt + 24`，`plus` = 可选头 magic 是 0x20B。

### 3.1 COFF 头（偏移相对 `coff`）

| 字段 | 偏移 | 宽度 |
|---|---|---|
| `machine` | 0 | u16 |
| `number_of_sections` | 2 | u16（原值，不截到 96） |
| `timestamp` | 4 | u32 |
| `pointer_to_symbol_table` | 8 | u32 |
| `number_of_symbols` | 12 | u32 |
| `size_of_optional_header` | 16 | u16 |
| `characteristics` | 18 | u16 |

### 3.2 可选头（偏移相对 `opt`）

| 字段 | PE32 | PE32+ | 宽度 |
|---|---|---|---|
| `opthdr_magic` | 0 | 0 | u16 |
| `linker_version.major` / `.minor` | 2 / 3 | 2 / 3 | u8 |
| `size_of_code` | 4 | 4 | u32 |
| `size_of_initialized_data` | 8 | 8 | u32 |
| `size_of_uninitialized_data` | 12 | 12 | u32 |
| `entry_point_raw` | 16 | 16 | u32 |
| `base_of_code` | 20 | 20 | u32 |
| `base_of_data` | 24 | **不设置**（Undef） | u32 |
| `image_base` | 28（u32） | 24（u64） | |
| `section_alignment` | 32 | 32 | u32 |
| `file_alignment` | 36 | 36 | u32 |
| `os_version.major` / `.minor` | 40 / 42 | 同 | u16 |
| `image_version.major` / `.minor` | 44 / 46 | 同 | u16 |
| `subsystem_version.major` / `.minor` | 48 / 50 | 同 | u16 |
| `win32_version_value` | 52 | 52 | u32 |
| `size_of_image` | 56 | 56 | u32 |
| `size_of_headers` | 60 | 60 | u32 |
| `checksum` | 64 | 64 | u32 |
| `subsystem` | 68 | 68 | u16 |
| `dll_characteristics` | 70 | 70 | u16 |
| `size_of_stack_reserve` | 72（u32） | 72（u64） | |
| `size_of_stack_commit` | 76（u32） | 80（u64） | |
| `size_of_heap_reserve` | 80（u32） | 88（u64） | |
| `size_of_heap_commit` | 84（u32） | 96（u64） | |
| `loader_flags` | 88 | 104 | u32 |
| `number_of_rva_and_sizes` | 92 | 108 | u32 |
| 数据目录起点 | 96 | 112 | 每项 8 字节：RVA u32 + 大小 u32 |

`u64` 字段按位转成 `i64`。

`entry_point` = `rva_to_offset(entry_point_raw)`；映射不到时官方的值是 **-1**（有定义，不是 undefined）。

### 3.3 数据目录

`data_directories[i].{virtual_address, size}`，`i` 从 0 到 `min(number_of_rva_and_sizes, 16) - 1`。某一项的 8 个字节不在文件内就停。

**其它地方取数据目录时不看 `number_of_rva_and_sizes`**：导入、导出、资源、调试目录直接读“数据目录起点 + 8 × 序号”处的 8 个字节，只要求它在文件内（官方的 `pe_get_directory_entry`）。序号：导出 0、导入 1、资源 2、调试 6。

### 3.4 节表

起点 = `opt + size_of_optional_header`，每项 40 字节，最多 `min(number_of_sections, 96)` 项，某一项不在文件内就停。

| 字段 | 偏移 | 宽度 |
|---|---|---|
| 名字 | 0 | 8 字节 |
| `virtual_size` | 8 | u32 |
| `virtual_address` | 12 | u32 |
| `raw_data_size` | 16 | u32 |
| `raw_data_offset` | 20 | u32 |
| `pointer_to_relocations` | 24 | u32 |
| `pointer_to_line_numbers` | 28 | u32 |
| `number_of_relocations` | 32 | u16 |
| `number_of_line_numbers` | 34 | u16 |
| `characteristics` | 36 | u32 |

`name`：8 个字节**去掉尾部的 NUL**（不是在第一个 NUL 处截断）。`.data\0\0x` 的名字是 8 个字节，中间带两个 NUL。

`full_name`：

1. `pointer_to_symbol_table == 0`，或名字第一个字节不是 `/` → 等于 `name`。
2. 否则：字符串表起点 = `pointer_to_symbol_table + number_of_symbols * 18`；序号 = 名字里 `/` 后面连续的十进制数字（最多到第 8 个字节）；从“起点 + 序号”处读一个 C 字符串：遇到 NUL 结束；遇到不可打印字节（0x20–0x7E 之外）或读出文件 → `Node::Undef`。

### 3.5 overlay

在节表循环里维护 `(highest_ofs, highest_siz)`，初值 `(0, 0)`：对每个节，若 `raw_data_offset + raw_data_size > highest_ofs + highest_siz`，就把这一对换成该节的。循环结束后 `end = highest_ofs + highest_siz`。

- `end != 0` 且 `文件长度 > end` → `overlay.offset = end`，`overlay.size = 文件长度 - end`。
- 否则两个都是 0。

不看 `size_of_headers`。没有节的 PE 两个都是 0。

---

## 4. Rich 头（任务 T21）

```
若 文件长度 < 64 或前两字节不是 "MZ" → 没有 Rich 头
nt = e_lfanew（u32）
若 nt > 文件长度 + 4 或 nt < 4 → 没有
p = nt - 4
循环（p >= 64）：                         # 往回找 "Rich"，每次退 4 字节
    若 p 处的 u32 == 0x68636952：
        key = (p + 4) 处的 u32；rich = p；p -= 4；跳出
    p -= 4
若 key == 0（含没找到）→ 没有
循环（p >= 64）：                         # 继续往回找解密后的 "DanS"
    若 (p 处的 u32 ^ key) == 0x536E6144：dans = p；跳出
    p -= 4
没找到 dans → 没有
len = rich - dans
```

读 u32 时越界 → 当作没有 Rich 头。

有 Rich 头时（`rich_signature` 是一个结构结点）：

| 字段 | 值 |
|---|---|
| `offset` | `dans` |
| `length` | `len` |
| `key` | `key` |
| `raw_data` | `data[dans..rich]` |
| `clear_data` | `raw_data` 每 4 字节与 `key`（小端）异或 |
| `version_data` | `len >= 16` 时：`clear_data` 从第 16 字节起每 8 字节取前 4 字节，拼起来。`len < 16` 时不放（Unknown） |

没有 Rich 头时：`rich_signature` 这个结点整个放 `Node::Undef`，这样它下面的字段和函数都是 undefined。

函数 `version(v)`、`version(v, toolid)`、`toolid(t)`、`toolid(t, v)`：

- `len < 16` → Undef。
- 把 `clear_data` 从第 16 字节起按 8 字节一项读：`id_version`（u32）、`times`（u32）。
- 一项的“版本”是 `id_version & 0xFFFF`，“工具号”是 `id_version >> 16`。
- 返回所有匹配项的 `times` 之和。只给了版本就只比版本，只给了工具号就只比工具号，两个都给就都要相等。

读数（`pe64_full`）：`version(0x7809) == 7`，`toolid(0x103) == 14`，`version(0x7809, 0x105) == 2`。

---

## 5. 导入表（任务 T22）

先放默认值 `number_of_imports = 0`、`number_of_imported_functions = 0`，然后：

```
(va, _) = 数据目录[1]；读不到或 va == 0 → 结束
off = rva_to_offset(va)；None → 结束
parsed = 0；total_funcs = 0
循环：desc = data[off .. off + 20]（读不到 → 结束）
    name_rva = desc[12..16]；若 name_rva == 0 → 结束
    若 parsed >= 16384 → 结束
    parsed += 1
    若 name_off = rva_to_offset(name_rva) 有值：
        dll = name_off 处的 C 字符串，按下面的规则判合法：
            最多看 n = min(文件长度 - name_off, 256) 个字节；
            每个字节必须在 0x20..=0x7E 且不是 " * < > ? |；
            必须在 n 个字节之内遇到 NUL，且长度 > 0
        合法 → funcs = 解析这个描述符的函数（见下）
               funcs 非空 → 把 (dll, funcs) 加入结果
    off += 20
number_of_imports = 结果里的 DLL 数
number_of_imported_functions = total_funcs
```

描述符的布局：`OriginalFirstThunk` @0、`Name` @12、`FirstThunk` @16。

解析一个描述符的函数：

```
t = rva_to_offset(OriginalFirstThunk)
若 t 是 None 或 Some(0) → t = rva_to_offset(FirstThunk)
若 t 是 None → 没有函数
宽度 w = plus ? 8 : 4；序号标志 = 最高位
idx = 0；attempts = 0
循环：thunk = t + idx * w 处的 w 字节整数（读不到 → 结束）
    若 thunk == 0 → 结束
    若 attempts >= 16384 或 total_funcs >= 16384 → 结束
    attempts += 1
    若最高位没置位（按名字导入）：
        若 o = rva_to_offset(thunk) 有值，且 data[o..o+3] 在文件内：
            name = (o + 2) 处的 C 字符串，最多 512 字节
    否则（按序号导入）：
        若除最高位和低 16 位之外没有别的位：ordinal = thunk & 0xFFFF；
            name = ordinals::ord_name(dll, ordinal)；有序号 = 是
    rva = FirstThunk + w * idx
    若 name 有值但不合法 → 跳过这一项（idx += 1，继续）
        合法 = 非空，且每个字符是字母、数字或 . _ ? @ $ ( ) < > 之一
    若 name 有值或有序号 → 记一个函数 { name, ordinal（有序号才有）, rva }；total_funcs += 1
    idx += 1
```

`rva_to_offset(thunk)` 的参数是完整的 64 位值（PE32+ 时不要截成 32 位）。

树里的形状：

```
import_details: Array[
    Struct{ library_name: Str, number_of_functions: Int,
            functions: Array[ Struct{ name: Str, ordinal: Int 或 Undef, rva: Int（rva 为 0 时 Undef）} ] } ]
```

函数（都**不区分大小写**地比 DLL 名和函数名，按 C 字符串比）：

| 调用 | 返回 |
|---|---|
| `imports(dll, func)` | 有匹配得 1，否则 0 |
| `imports(dll)` | 该 DLL 的函数个数（多个同名 DLL 相加） |
| `imports(dll, ordinal)` | 该 DLL 有“按序号导入且序号相等”的函数得 1，否则 0 |
| `imports(flags, …)` 三个重载 | `flags & 1` 时同上；`flags & 2`（延迟导入）时：布尔型重载在普通导入已命中时仍返回 1，否则 `Unknown`；计数型重载返回 `Unknown` |
| `imports(正则, 正则)` 及带 flags 的版本 | `Unknown`（官方要求“匹配长度大于 0”，现有正则引擎拿不到匹配长度） |
| `import_rva`、`delayed_import_rva` | `Unknown` |

不是 PE 时这些函数都是 undefined；`modreg` 已经处理（根结点 `missing_undef: true` 下，函数的 owner 走不到）。**但要注意**：函数的 owner 是模块根，根在非 PE 时是存在的结构结点，所以 `call` 开头必须先看 `is_pe`：为 0 就返回 `Val::Undef`。

读数（`pe64_full`）：3 个 DLL（`KERNEL32.dll` 2 个函数、`WS2_32.dll` 3 个、`misc.dll` 1 个）。`evil*.dll` 因为名字非法被跳过；`bad name!` 因为函数名非法被跳过；`WS2_32.dll` 的序号 3、115、999 分别得到 `closesocket`、`WSAStartup`、`ord999`；`misc.dll` 的 `OriginalFirstThunk` 是 0，走 `FirstThunk`。

---

## 6. 导出表（任务 T23）

先放默认值 `number_of_exports = 0`，然后严格按下面的顺序。**每个“结束”都是提前返回，前面已经放进树里的字段保留。**

```
(va, size) = 数据目录[0]；读不到或 va == 0 → 结束
start = rva_to_offset(va)；None → 结束
dir = data[start .. start + 40]；读不到 → 结束
n = min(NumberOfFunctions, 16384)
base = Base
放 export_timestamp = TimeDateStamp
name_off = rva_to_offset(Name)
若 name_off 有值且 > 0 → 放 dll_name = 该处的 C 字符串（到文件尾为止）
bound = 文件长度 - (name_off 有值 ? name_off : -1)      # 用回绕算术；name_off 是 None 时等于 文件长度 + 1
若 n * 4 > bound → 结束
若 NumberOfNames > 0：
    names = rva_to_offset(AddressOfNames)；None → 结束
    若 NumberOfNames * 4 > 文件长度 - names → 结束
ords = rva_to_offset(AddressOfNameOrdinals)；None → 结束
若 ords 之后的剩余字节 < 2 * n → 结束
funcs = rva_to_offset(AddressOfFunctions)；None → 结束
若 funcs 之后的剩余字节 < 4 * n → 结束
n_names = min(NumberOfNames, n)
对 i 从 0 到 n - 1：
    ordinal = base + i
    rva = funcs 处第 i 个 u32
    off = rva_to_offset(rva)
    若 off 有值 且 start < off < start + size：
        forward_name = off 处的 C 字符串，最多 512 字节；**不放 offset**
    否则：
        offset = off 有值 ? off : Undef
    若有 names：找第一个 j（0 ≤ j < n_names）使得 ords 处第 j 个 u16 == i：
        若 o = rva_to_offset(names 处第 j 个 u32) 有值且 > 0 → name = 该处的 C 字符串，最多 512 字节
number_of_exports = n
```

导出目录的布局：`TimeDateStamp` @4、`Name` @12、`Base` @16、`NumberOfFunctions` @20、`NumberOfNames` @24、`AddressOfFunctions` @28、`AddressOfNames` @32、`AddressOfNameOrdinals` @36。

每个导出项：`Struct{ ordinal, rva, offset（或没有）, forward_name（或没有）, name（或没有）}`。没有的字段这里要**显式放 `Node::Undef`**（官方是 undefined，不是弃权）。

函数（n = `number_of_exports`）：

| 调用 | 返回 |
|---|---|
| `exports(name)` | n == 0 得 0；否则有名字**不区分大小写**相等的导出得 1，否则 0 |
| `exports(正则)` | n == 0 得 0；否则有名字能被正则搜到的得 1，否则 0。正则在引擎子集外 → `Unknown` |
| `exports(ordinal)` | n == 0 得 0；**`ordinal == 0` 或 `ordinal > n` 得 0**；否则有序号相等的得 1，否则 0 |
| `exports_index(…)` 三个重载 | 同样的匹配规则，返回下标；n == 0、没找到、`ordinal == 0 或 > n` → Undef |

`exports(ordinal)` 拿序号和**导出个数**比，这是官方的写法。`pe64_full` 有 3 个导出、序号 5–7，于是 `pe.exports(5)` 不命中（5 > 3），向量里有。

---

## 7. 资源与版本信息（任务 T24）

### 7.1 遍历

```
(va, _) = 数据目录[2]；读不到或 va == 0 → 没有资源
base = rva_to_offset(va)；None → 没有资源
根目录 = data[base .. base + 16]；读不到 → 没有资源
放 resource_timestamp（@4，u32）、resource_version.major（@8，u16）、resource_version.minor（@10，u16）
type = id = lang = -1                       # 三个整数，整个遍历过程共用
遍历(根目录偏移 = base, 层 = 0, type_str = 无, name_str = 无, lang_str = 无)
```

目录头 16 字节：`Characteristics` @0（u32）、`NumberOfNamedEntries` @12（u16）、`NumberOfIdEntries` @14（u16）。目录项 8 字节：`Name` @0、`OffsetToData` @4。

```
遍历(dir, 层, type_str, name_str, lang_str):
    若 Characteristics != 0 或 NumberOfNamedEntries > 32768 或 NumberOfIdEntries > 32768 → 返回
    total = NumberOfNamedEntries + NumberOfIdEntries
    若 data[dir + 16 .. dir + 16 + total * 8] 不在文件内 → 返回
    e = dir + 16
    对 i 从 0 到 total - 1：
        若 e 处的 OffsetToData == 0 → continue        # 注意：e 不前进。等于这一层到此为止
        按层：0 → type = Name；type_str = 名字串(e)
              1 → id = Name；  name_str = 名字串(e)
              2 → lang = Name；lang_str = 名字串(e)
        若 OffsetToData 最高位置位 且 层 < 2：
            sub = base + (OffsetToData & 0x7FFFFFFF)
            若 data[sub .. sub + 16] 在文件内 → 遍历(sub, 层 + 1, type_str, name_str, lang_str)
        否则：
            d = base + (OffsetToData & 0x7FFFFFFF)
            若 data[d .. d + 16] 在文件内，且 Size（@4）在 1..=0x3FFFFFFF：
                收集(d, type, id, lang, type_str, name_str, lang_str)
        e += 8
```

“`continue` 时 `e` 不前进”是官方代码的实际行为，必须照抄：遇到 `OffsetToData == 0` 的项之后，这一层剩下的迭代都看的是同一项，等于停止。

`名字串(e)`：`Name` 最高位置位时，`s = base + (Name & 0x7FFFFFFF)`；`data[s..s+2]` 要在文件内；长度 `L`（u16）> 1000 → 无；`data[s .. s + 2 + 2L]` 要在文件内；结果是 `data[s+2 .. s+2+2L]`（UTF-16LE 的原始字节）。最高位没置位 → 无。

### 7.2 收集一项

已收集满 65536 项就不再收集。数据项 16 字节：`OffsetToData`（RVA）@0、`Size` @4。

| 字段 | 值 |
|---|---|
| `rva` | `OffsetToData` |
| `offset` | `rva_to_offset(rva)`；None → Undef |
| `length` | `Size` |
| `type_string` / `type` | 有 `type_str` → 放 `type_string`，`type` 放 Undef。没有 → `type != -1` 时放 `type`，`type_string` 放 Undef |
| `name_string` / `id` | 同上 |
| `language_string` / `language` | 同上 |

`type == 16`（版本资源）时再做 7.3。最后 `number_of_resources` = 收集到的项数。

### 7.3 版本信息

三个辅助（UTF-16LE 串，`p` 是文件偏移）：

- `宽串等于(p, "TEXT")`：从 p 起每 2 字节一个单元，依次是 TEXT 的每个 ASCII 字符（高字节为 0），紧接着一个 `00 00`。
- `宽串长度(p)`：单元数，数到第一个 `00 00` 为止（越界当作失败）。
- `宽串转窄(p, 最多 n 个字符)`：依次取每个单元的**低字节**，遇到低字节为 0 的单元停（不看高字节），最多 n 个。
- `宽串在文件内(p)`：从 p 起能在文件内找到一个 `00 00` 单元。
- `对齐(x)` = `(x + 3) & !3`。

每个结点的头 6 字节：`Length`（u16）@0、`ValueLength`（u16）@2、`Type`（u16）@4，键从 @6 开始。

```
vi = rva_to_offset(数据项的 OffsetToData)；None → 返回
data[vi .. vi + 6] 与 data[vi + 6 .. vi + 6 + 32] 要在文件内，否则返回
若不满足 宽串等于(vi + 6, "VS_VERSION_INFO") → 返回
p = vi + 92
当 data[p + 6 .. p + 6 + 24] 在文件内 且 宽串等于(p + 6, "VarFileInfo") 且 p 处 Length != 0：
    p += 对齐(Length)
当 data[p + 6 .. p + 6 + 30] 在文件内 且 宽串等于(p + 6, "StringFileInfo") 且 p 处 Length != 0：
    table = p + 36
    p += 对齐(Length)
    当 data[table .. table + 6] 在文件内 且 宽串在文件内(table + 6) 且 table 处 Length != 0 且 table < p：
        s = table + 对齐(6 + 2 * (宽串长度(table + 6) + 1))
        table += 对齐(table 处 Length)
        当 data[s .. s + 6] 在文件内 且 宽串在文件内(s + 6) 且 s 处 Length != 0 且 s < table：
            v = s + 对齐(6 + 2 * (宽串长度(s + 6) + 1))
            若 宽串在文件内(v)：
                key = 宽串转窄(s + 6, 63)
                value = s 处 ValueLength == 0 ? 空串 : 宽串转窄(v, 255)
                version_info[key] = value          # 同名键：覆盖已有的值，位置不变
                version_info_list 追加 { key, value }
            s += 对齐(s 处 Length)
number_of_version_infos = version_info_list 的长度
```

读数（`pe64_full`）：4 项。`ProductName` 的原值是 `café 中`，转窄之后是 `caf\xe9 -`（`é` 的低字节是 0xE9，`中` 是 U+4E2D，低字节 0x2D 是 `-`）。`Empty` 的值是空串，有定义。

### 7.4 函数

- `locale(x)`：任一资源的 `language & 0xFFFF == x` 得 1，否则 0。
- `language(x)`：任一资源的 `language & 0xFF == x` 得 1，否则 0。
- 资源的 `language` 是 Undef（用名字串给出）时，官方拿哨兵位模式参与比较：把它当成 `0xFFFABADAFABADAFF` 再取低位。

---

## 8. 其余（任务 T25）

| 名字 | 规则 |
|---|---|
| `is_dll()` | `characteristics & 0x2000`。**返回 0x2000 或 0，不是 1**（循环计数时看得出来，向量里有 `for 8193 i in (0..0) : (pe.is_dll())`） |
| `is_32bit()` / `is_64bit()` | 按可选头 magic，返回 0 / 1 |
| `section_index(name)` | 第一个“名字按 C 字符串比较相等”的节的下标；实参和节名都在第一个 NUL 处截断。没有 → Undef |
| `section_index(addr)` | 第一个满足 `raw_data_offset <= addr < raw_data_offset + raw_data_size` 的节的下标。没有 → Undef |
| `rva_to_offset(rva)` | `rva::rva_to_offset`，实参按位转成 u64；None → Undef |
| `calculate_checksum()` | 见下 |
| `pdb_path` | 见下 |

`calculate_checksum()`：把文件按 4 字节小端一组累加成 64 位和，跳过校验和字段所在的那一组（文件偏移 `opt + 64`，只在这个偏移是 4 的倍数时才会被跳过）；末尾不足 4 字节的按小端补进去；每加一组后若和超过 32 位就折叠（`(s & 0xffffffff) + (s >> 32)`）；最后 `s = (s & 0xffff) + (s >> 16)`，`s += s >> 16`，`s &= 0xffff`，再加上文件长度。

`pdb_path`：数据目录[6]，大小或 RVA 为 0 → 没有。`off = rva_to_offset(RVA)`。对每个 28 字节的调试目录项（`Type` @12、`AddressOfRawData` @20、`PointerToRawData` @24），只看 `Type == 2` 的：`cv = rva_to_offset(AddressOfRawData)`（`AddressOfRawData` 非 0 时）；`cv` 没有或为 0 且 `PointerToRawData != 0` → 用 `PointerToRawData`；仍然没有或为 0 → 下一项。`cv` 处的 4 字节签名：`RSDS` → 路径在 `cv + 24`；`NB10` → `cv + 16`；`MTOC` → `cv + 20`。路径是 C 字符串，长度小于 4096 才算数；取到一个就结束。没有 → Undef。

---

## 9. 不实现的（保持弃权）

这些在 schema 里有，解析和类型检查照做，求值时树里没有它们，自动是 `Unknown`：

`signatures[*]`、`number_of_signatures`、`is_signed`、`imphash()`、`delayed_import_details[*]`、`number_of_delayed_imports`、`number_of_delayed_imported_functions`、`import_rva()`、`delayed_import_rva()`、正则版的 `imports()`。

每一项都要在 `docs/YARA-EXPR.md` 的缺口表里登记一行，写明原因。

---

## 10. 怎么验证

1. `cargo test -p trit-re --test yara_expr_vectors -- --nocapture`：看 `pe_fields.tsv` 的错判是否为 0，弃权数是否在下降。
2. 真样本：`tools/gen_value_probes.py` 对一个样本目录生成逐字段的精确值规则，再用仓库的 `scripts/yara_diff.py` 对拍。每做完一段跑一次，DIFF 必须是 0。
3. 对不上时的处理顺序：先对照本文件的伪代码逐行查；再去读 `pe.c` 里对应的函数；还是对不上，就让这个字段在那种情况下不进树（弃权），并登记原因。**不要为了让某个样本通过而加特判。**
