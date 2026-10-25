# trit-unsafe

用三值逻辑（K3）审 `unsafe`：每个点判 **Safe / Unknown / Risky**，按 crate 用 K3 的「或」合成。

## 为什么是三值

审计里大部分点既证明不了安全、也证明不了危险。二值化只有两种结局：全报（噪声淹死人）或全不报（漏）。三值把"说不清"显式留成 `Unknown`：

| 判定 | 映射 | 含义 |
|---|---|---|
| Safe | `Ternary::False` | 高危形态 + `// SAFETY:`/`# Safety` 说明 + 附近的断言/边界检查都在 |
| Unknown | `Ternary::Unknown` | 只有其中一项证据（或没有已知高危形态） |
| Risky | `Ternary::True` | 高危形态 + 说明和检查都没有 |

合成用 `trit_core::Algebra::Kleene.or`：`F∨U=U`、`U∨T=T`（取证据最强的那个）。
所以一个文件里只要有一处 Risky，整个 crate 就是 Risky。

## 用法

```bash
# 只看本仓
trit-unsafe scan --root .

# 连 Cargo.lock 里的依赖一起扫（从 ~/.cargo/registry 读源码）
trit-unsafe scan --root . --deps --top 20

# 排除机器生成的绑定（它们的每个 FFI 调用都会命中外联规则）
trit-unsafe scan --root . --deps --exclude windows --exclude linux-raw-sys --exclude spirv
```

`--strict`：出现 Risky 就退出 1。工作区目前是干净的（见下），可以直接当门。

## 识别什么

- **站点**：`unsafe { ... }`、`unsafe fn`、`unsafe impl`、`unsafe trait`
- **高危形态**：`transmute`、`from_raw_parts`、`get_unchecked`、`set_len`、`assume_init`、
  `copy_nonoverlapping`、`ptr::read/write`、`static mut`、`extern "C"`、`unreachable_unchecked`、
  `new_unchecked`、`align_to`、`offset_from`
- **证据**：往上 30 行注释里的 `SAFETY` / `Safety`（含 `/// # Safety` 文档段）；
  ±15/±20 行内的 `assert`/`debug_assert`/`is_null`/`checked_`/`len` 比较/`align`/`try_into`/`matches!` 等
- **签名级** `unsafe fn/impl/trait` 一律 `Unknown`（它只是承诺面），未文档化的签名单独计数
  `undocumented_unsafe_signatures`；测试/示例/基准里的 Risky 单独计数，不算进 strict

## 实测（本机，2026-10-25）

| 范围 | 文件 | 站点 | Safe | Unknown | Risky |
|---|---|---|---|---|---|
| 本仓 `crates/` | 209 | 4 | 0 | 4 | **0** |
| 本仓 + 193 个依赖 crate | — | 224,696 | 312 | 180,997 | 43,376 |

**风险质量集中在绑定 crate**：`windows` 184,979 站点 / 38,042 Risky，`linux-raw-sys` 5,900/3,730，
`spirv` 414/414 —— 这些是机器生成的 FFI 包装，命中的是"每个 raw 调用都没写 SAFETY"。

排除它们之后才是值得人工看的：

| crate | 站点 | Risky |
|---|---|---|
| `ash`（Vulkan 绑定，手写） | 7,439 | **856** |
| `rustix` | 1,584 | 53 |
| `allocator-api2` | 214 | 20 |
| `tokio` | 868 | 15 |

配合 `cargo audit`（本机离线库）：本仓依赖**已知漏洞 0**，1 个 unmaintained。

## 限制（重要）

- **`Safe` 不是 UB 证明**：只表示"常见防护证据都在"。要证伪得靠 Miri/loom/模糊测试。
- 词法级扫描：不展开宏、不做跨函数推理；字符串/注释已抹除（`r#"..."#` 已处理，字符字面量按普通字符处理）。
- 生成绑定会把 Risky 数拉爆——用 `--exclude` 或看密度，不要看绝对值。
- 不替代 `cargo clippy -W clippy::undocumented_unsafe_blocks`（后者有 AST 精度，但只能跑工作区）。
