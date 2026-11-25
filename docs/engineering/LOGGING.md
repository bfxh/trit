# LOGGING：统一运行时日志

> 定位：全仓诊断输出的唯一开关。回答三个问题：这条信息该不该出现、出现时带什么级别、
> 写到哪里（终端还是文件、文本还是 JSONL）。
> 边界：只做日志，不做指标采集、不做分布式追踪、不做日志轮转与压缩；`TRIT_LOG_FILE`
> 打开后文件会一直长，由调用方自己管生命周期。
> 状态：**已落地**（`cargo test -p trit-log` 8/8 + doctest；全仓 `eprintln!` 诊断
> 迁完，长任务进度行改 `info/debug`，结果输出仍走 stdout）。

## 1. 为什么按「诊断 / 结果」分家

迁移前全仓有 625 个 `println!`、240 个 `eprintln!`（`crates/*/src`），两类东西混在
一起：训练进度、GPU 适配器、跳过原因和 JSON、表格、路径都往同一个 stdout 写。后果是

- 脚本想解析结果时，进度行混在里面；
- 长任务想留一份日志给明天看，得自己 `tee`，而 `tee` 不区分级别；
- 出问题想只看错误，只能 `2>/dev/null` 或 `grep` 猜格式。

所以口径定死：**stdout 是结果，stderr 是诊断**。结果（表格、JSON、清单、裁决）
继续 `println!`；诊断（错误、警告、进度、适配器、续跑、跳过）走 `trit_log`。
默认级别 `info`，迁移前打得出来的诊断，迁移后默认仍然打得出来。

## 2. 接口

```rust
trit_log::init();                    // 可选：进程里调一次；不调也会在第一条日志时懒初始化
trit_log::info!("加载模型 {}", path);
trit_log::debug!("逐帧 mid 转储 {}", n);
trit_log::warn!("跳过 {}: {}", name, why);
trit_log::error!("读取失败: {e}");
```

宏用 `module_path!()` 当 target，过滤表按 target 前缀匹配。手写调用也可以：
`trit_log::log(Level::Info, "trit_parts::train", format_args!("..."))`。

| 环境变量 | 取值 | 说明 |
| --- | --- | --- |
| `TRIT_LOG` | `error/warn/info/debug/trace/off`，或 `target=level` 逗号表 | 最长前缀胜出；`-` 按 `_` 归一 |
| `TRIT_LOG_FILE` | 路径 | 追加写；不设写 stderr |
| `TRIT_LOG_FORMAT` | `text`（默认）/`jsonl` | JSONL 一行一个对象：`ts/level/target/msg` |

例子：

```bash
TRIT_LOG=warn cargo run -p trit-parts --bin parts-train -- ...
TRIT_LOG=trit_parts=debug,trit_parts::train=trace TRIT_LOG_FILE=train.log \
  cargo run -p trit-parts --bin parts-train -- ...
TRIT_LOG_FORMAT=jsonl TRIT_LOG_FILE=train.jsonl ...   # 给下游 jq / pandas
```

文本行格式：`2026-11-24T06:12:03.456Z INFO  trit_parts::train - 续跑自 ...`；UTC、毫秒。
JSONL 转义引号/反斜杠/换行/控制字符，保证一行一条、可直接 `jq`。

## 3. 级别怎么选

| 级别 | 用在哪 | 例子 |
| --- | --- | --- |
| `error` | 本次操作失败、结果不可信、要退出 | 权重读不了、清单不一致、审计链校验失败 |
| `warn` | 能继续但环境不对、功能被跳过 | 没有 GPU adapter、未启用 feature、缺可选文件 |
| `info` | 长任务的进度与关键状态 | 续跑自 step N、step/loss、ckpt、已存 |
| `debug` | 排查时才要的过程细节 | `DEPTH_DEBUG` 下的 holdout 清单、mid 转储 |
| `trace` | 逐条数据流 | 当前没有调用点；留给后续 |

默认 `info` 是为了不缩水；想让长任务安静，`TRIT_LOG=warn`。`off` 全关。

## 4. 迁移口径与现状

- **已迁**：`crates/*/src` 里 192 处 `eprintln!` 诊断 → `error!/warn!`（按语义分）；
  `trit-parts` 训练/深度/纹理长任务的 41 处进度叙述 → `info!/debug!`。
- **保留**：48 处 `eprintln!`——41 处用法/帮助文本（含 14 处多行 `用法:\n...` 整块），
  以及 7 处以 `FAIL ` 开头的失败行。后者是 stderr 契约（`trit-wspace` 等 CLI 测试
  断言 `stderr` 以 `FAIL ` 开头），加了时间戳/级别前缀就不再是契约，所以留在原地。
- **保留**：`println!` 里的结果输出（表头、逐行读数、JSON、清单路径、裁决、用法文本）。
  它们是对外输出契约，迁成日志会把脚本和测试一起改坏。
- **测试里**的 `eprintln!` 保留：测试输出不是运行时日志。
- 判定只看一句话：**这行是给人看状态的，还是给程序/人当结果用的？** 前者日志，后者 stdout。

诚实边界：

- 日志器单锁串行写；高并发下是正确性优先，不承诺吞吐。
- 文件写入失败不 panic、不回退 stderr（调用方传错路径自己承担），`set_file` 会把错误返回。
- 没有轮转、没有上限、没有压缩；`TRIT_LOG_FILE` 指到哪就写到哪。
- 没有 trace/span 关联 ID，也不采集指标；这些不在本 crate 范围。
- 时间戳是 UTC，不是本地时区；看日志时按 UTC 对表。

## 5. 验证

```bash
cargo test -p trit-log --offline
cargo clippy -p trit-log --offline --all-targets -- -D warnings
TRIT_LOG=debug cargo run -p trit-parts --bin parts-train -- --help   # 级别过滤手验
```

`cargo test -p trit-log` 覆盖：级别与别名解析、`TRIT_LOG` 裸级别与 `target=level`
过滤（最长前缀胜出）、文本行格式与 UTC 换算（含闰日）、JSONL 转义、`off` 全关。
