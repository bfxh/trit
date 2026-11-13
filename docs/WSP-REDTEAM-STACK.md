# trit 容器栈红队：结构化攻击读数（2026-11-14）

攻击对象是**本仓自己的软件**（授权红队）。方法不是随机 fuzz，而是**带着解析器的
逆向知识构造结构化恶意输入**：所有容器尾部校验和都是无密钥 FNV-1a，攻击者可以
自行计算 ⇒ 「校验和合法但内容恶意」是必须假设的基线攻击。攻击套件固定在
`crates/trit-wspace/tests/redteam.rs`（9 条，CI 常跑），每条对应下面一个发现。

## 发现与修复（同 PR 修复）

| # | 发现 | 严重度 | 攻击成本 | 修复 |
|---|---|---|---|---|
| R1 | **WSP2 行数分配炸弹**：`from_wsp2` 在校验和之后直接 `Vec::with_capacity(rows)`，rows 取自容器头（u32）。15 字节的 `rows=u32::MAX` 文件让 64 位平台试图分配 ~16GB ⇒ `handle_alloc_error` abort | 高（DoS） | 15 B | 分配前护栏：`rows × 4` 必须 ≤ 剩余字节（每行至少 4 B scale），否则 `Wire` 错误 |
| R2 | **WSP3 声明长度炸弹**：`from_wsp3` 把头部 `raw_len`（u64）直通 `rle_decode` 的 `with_capacity`。13 字节文件声明 2^40 ⇒ TB 级分配 abort | 高（DoS） | 13 B | 护栏：`raw_len ≤ 压缩数据 × 130`（PackBits 理论最大展开率），超出报「解压上界」 |
| R3 | **WSP4 符号数炸弹**：`huffman_decode` 的 `with_capacity(raw_len)` 同型。266 字节（256 码长表 + 头）声明 2^40 符号 ⇒ abort | 高（DoS） | 266 B | 护栏：每个符号至少 1 bit ⇒ `raw_len ≤ 位流字节 × 8` |
| R4 | **WSPM blob 偏移溢出**：`start + len`（均取自 entry，u64）在 usize 上直接相加——debug 构建溢出 panic，release 回绕后可能落到错误区间 | 中 | 修改 8 字节 | `u64::checked_add` 后再与 `payload_end` 比较，溢出显式报错 |
| R5 | **manifest 路径穿越读**：`pack-model` / `export-gguf` / `export-safetensors-dir` 对清单 `tensors[].file` 直接 `Path::join`——`Path::join` 遇绝对路径直接覆盖前缀，`../` 也能走出目录 ⇒ 攻击者给的清单能让打包命令把**任意文件**打进容器 | 中（信息泄露） | 改 1 个 JSON 字段 | 新增 `cli/safe_path.rs`：只接受清单目录内的普通相对路径（拒绝绝对路径/`..`/空串），三个读取点统一接入 |
| R6 | 解析器里残留的 `expect`（截断场景）逐一复核：均在长度前置检查之后（`bytes.len() < HEADER+8` 等），不可达 ⇒ 记录在案，不改 | 信息 | — | 无 |

未改的近似点（已自然安全）：`rle_decode` 逐条 `checked_add` + 超长即断；
`huffman_decode` 位流 `bits.get(pos/8)` 显式截断错误；`canonical_codes` Kraft
不等式校验在解码侧拦截恶意码表；`wspm` 的 `count.min(1024)` 限制 capacity、
`bias_ref` 自引用/越界/重复名/零形状在 blob 解码前拒绝。

## 攻击者模型与边界

- **在范围内**：能投递容器/清单文件给 trit CLI 或库的攻击者（供应链投毒、
  下载的模型包、共享目录）；目标是崩溃（DoS）、OOM abort、越界读、把本地
  文件走私进容器。
- **不在范围内**：内存安全（全仓 `#![forbid(unsafe_code)]`，无 unsafe ⇒ 无
  RCE 级原语）；密码学校验和绕过（FNV 本来就不是抗篡改承诺，防篡改走审计
  哈希链，见 `docs/audit` 相关文档）；有密钥场景的对抗。

## 复算

```bash
cargo test -p trit-wspace --test redteam
```

修复前用同一文件可在 Windows 上复现 abort（R1/R2/R3 在 Linux overcommit 下
表现为分配成功后优雅报错或慢速 OOM，平台差异本身也是读数的一部分）。

## 给后续红队的入口

- 模型侧（比特翻转攻击三值工件）见 `docs/WSP-REDTEAM-MODEL.md`；
- 审计日志的篡改/重放已有哈希链与重放测试覆盖，未在本轮展开；
- 写锁是 advisory（`docs/WSP-WRITE-LOCK.md`），本地多用户场景不在威胁模型内。
