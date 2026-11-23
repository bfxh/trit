# trit-doctor：doctor 的主入口（七面 + Belnap 四值）

`publish = false`。复刻 `tools/doctor.py` 的七个观测面，输出同一份 `--json` schema；状态层用
`trit-belnap::B4`，不碰哈希与 JSON 编码。

> 状态：**主入口已切到 `trit-doctor`**。`tools/doctor.py` 保留为 reference oracle 与无 Rust
> 二进制的回退，不删除、不改写；两条实现独立，A/B 继续当回归。

## 七面

| face | 证据 | 读什么 |
|---|---|---|
| `selftest` | `.agent/last-selftest.json` | 自检汇总；缺失/坏 JSON → UNAVAILABLE |
| `ledger` | `docs/ERRORS.md` + `tools/selftest.ps1` + registry | 固化列断言号、幽灵号、反向漂移 |
| `registry` | `.agent/checks/registry.json` | 命令/scope 存在性，不执行检查 |
| `backlog` | `docs/ERRORS.md` | D 节 + 欠账/待办/未固化关键词 |
| `git` | `git status` / `for-each-ref` | dirty、分支、未推送；非仓库 → UNAVAILABLE |
| `gates` | `scripts/privacy_scan.py`、`scripts/docs_hygiene.py` | 只记退出码；复用 `.agent/doctor-cache.json` |
| `simchain` | `.agent/last-simchain.json` | 违规数；缺失/坏 JSON/自相矛盾 → UNAVAILABLE |

## 状态格

| doctor | Belnap | 含义 |
|---|---|---|
| `OK` | `T` | 证据在场且为真 |
| `WARN` | `B` | 同时有绿/红证据（债务/未固化） |
| `FAIL` | `F` | 证据在场且为假 |
| `UNAVAILABLE` | `N` | 证据缺席，不能判真假 |

聚合用 `B4::and`：最弱证据决定整体，不是平均分。`FAIL` 不改变退出码——观测面不是闸。

## 跑

```bash
# 主入口：安装一次后直接跑
cargo install --path crates/trit-doctor --locked
trit-doctor --root . --json

# 不安装
cargo test -p trit-doctor
cargo build --release -p trit-doctor
# 路径以 cargo metadata 的 target_directory 为准
<target>/release/trit-doctor --root . --json
pwsh tools/doctor_ab.ps1 -Runs 7      # 同机 A/B：暖态 < 0.15 s + JSON 逐字节
```

同机读数（n=7，暖态）：Python 0.673 s / Rust 0.110 s（6.1×）；冷态 `--refresh`（n=3）：
1.393 s / 1.109 s（1.26×）。真仓暖/暖与冷/冷 JSON 除顶层 `at` 外逐字节一致。

## 边界

- 坏 JSON 的错误文案不逐字对齐：serde 与 Python `JSONDecodeError` 措辞不同；状态仍为 UNAVAILABLE。
- `git` / gates 缓存需要 `git` 在 PATH 上（`cache_allowed` 用 `git check-ignore` 判断能否落盘）。
- 暖态依赖两个缓存：`.agent/doctor-cache.json`（与 Python 共用）与
  `.agent/trit-doctor-git-cache.json`（本 crate 专用，已进 `.gitignore`）。
- 内存峰值没有稳定读数；二进制 release 约 4.4 MiB。
- 不读跨机状态，不承诺防篡改；`--refresh` 只强制重跑 gates。

## 模块

`status.rs`（四值格与聚合）、`face.rs`（Face/Report）、`time.rs`（UTC）、`ledger.rs`、`registry.rs`、
`gates.rs`（指纹与门缓存）、`git.rs`（git 面与暖缓存）、`selftest.rs`、`simchain.rs`、`lib.rs`（七面编排）、
`main.rs`（CLI 与 JSON/文本输出）。
