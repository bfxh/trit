# SECURITY-SCAN：静态/动态/运行时扫描的统一门

> 定位：同一份报告回答"这个项目现在有哪些安全发现、哪些扫描器没跑、为什么"。
> 不是把 100 个安全工具全装进来；是把已有引擎、可选外部工具和缺口说清楚。
> 代码：`tools/security_gate.py`；回执：`security-gate/1`；审计：`tools/agent_audit.py`。

## 1. 为什么不是一个工具名列表

"跑了 clippy"不等于"做了安全扫描"：lint、依赖 CVE、密钥、隐私内容、unsafe、
fuzz、运行时 MCP 配置是不同证据面。`security_gate` 把每个 adapter 的
`status` 固定为 `pass / findings / unavailable / not_applicable / error / planned`，
再用 `findings` 和 `gaps` 给出唯一 verdict。缺一个必需扫描器时不是"跳过"，是
`gap`；默认 `--allow-gaps` 关闭，所以必需缺口直接 exit 1。

## 2. 适配器矩阵

| id | 类别 | 命令/来源 | 何时适用 | 失败语义 |
|---|---|---|---|---|
| `secrets` | static | `scripts/secret_scan.py --root . --json` | 项目有该脚本则用项目副本，否则用 trit 本仓引擎 | high，必需 |
| `privacy` | static | `scripts/privacy_scan.py --root . --json` | 同上；只把 `new` 判 high；baseline `details` 不重复报 | high，必需 |
| `rust-deps` | deps | `cargo audit --json` | `Cargo.lock` 存在 | vulnerability high；unmaintained medium；unsound high |
| `rust-lint` | lint | `cargo clippy --workspace --all-targets --locked -- -D warnings` | `Cargo.toml` 存在 | medium，不单独阻断 high 门 |
| `trit-unsafe` | static | `cargo run -q -p trit-unsafe -- scan --root . --strict` | `crates/trit-unsafe` 存在 | strict 非零 high |
| `semgrep` | static | `semgrep scan --config auto --json --quiet .` | 命令存在 | 按 `extra.severity` ERROR/WARNING/INFO |
| `bandit` | static | `bandit -r . -f json -q` | Python 项目 + 命令存在 | HIGH/MEDIUM/LOW |
| `trivy` | deps | `trivy fs --format json --scanners vuln,secret,misconfig --quiet .` | 命令存在 | CRITICAL/HIGH/... |
| `govulncheck` | deps | `govulncheck ./...` | `go.mod` + 命令存在 | 非零 high |
| `npm-audit` | deps | `npm audit --json` | `package-lock.json` + npm | 按 npm severity |
| `cargo-fuzz` | dynamic | `cargo fuzz list` 后逐个 `cargo fuzz run <target> -- -runs=1000` | `fuzz/Cargo.toml` | 无 target = not_applicable；任一 target 崩溃/超时 high；构建失败 = 必需 gap |
| `mcp-audit` | runtime | `cargo run -q -p trit-procscan -- mcp <config> --timeout-ms 8000 --strict` | 仓内 MCP 配置 | 非零 high |
| `security.commands` | 用户定义 | argv 列表，超时/severity/required 可配 | 用户显式配置 | 按配置；绝不经过 shell |

外部工具不是自动安装的。CI 只在 `security gate (quick, fail-closed)` 里装
`cargo-audit`；Semgrep/Trivy 等由用户或独立 job 提供。原因：安全工具的供应链
本身也是攻击面，未审核的自动安装会把"扫描"变成"扩大权限"。

## 3. 命令

```bash
python tools/security_gate.py --root . --mode quick --out target/agent-security/security-gate.json
python tools/security_gate.py --root . --mode all --fail-on critical,high
python tools/security_gate.py --root . --mode static --scanner rust-deps --scanner semgrep
python tools/security_gate.py --self-test
```

`quick` = secrets + privacy + 依赖审计；`static` 加 lint/unsafe/SAST；
`dynamic` 跑 fuzz target；`runtime` 跑 MCP 配置体检；`all` 全开。
`--list` 只打印计划和缺失原因，不执行。

## 4. 回执与一次性审计

项目里想默认开启，就在 `<root>/.agent/settings.json` 写：

```json
{"version": 1, "security": {"enabled": true, "mode": "quick",
  "fail_on": ["critical", "high"], "allow_gaps": false}}
```

用户级同名字段写到 `%APPDATA%\trit\agent\settings.json`（POSIX
`$XDG_CONFIG_HOME/trit/agent/settings.json`），项目设置覆盖用户设置；临时改一档用
`--security-mode static`，不改文件。

`agent_verify --security` 在 run 之前调用同一个门，回执里只写：

```json
"security": {"verdict": "pass", "exit": 0, "mode": "quick",
             "findings": 0, "blocking": 0, "gaps": 0,
             "report": "target/agent-security/security-gate.json",
             "sha256": "..."}
```

`python tools/agent_audit.py --root . --receipt full.json` 就能一次性审计：回执
`security.report` 指向的扫描报告会被自动拉取，且回执里的 `security.sha256` 必须
与报告文件一致（不一致判红）。也可以显式 `--scan target/agent-security/security-gate.json`
覆盖自动发现。compact 视图、delta 信封、exit≠0、root 失配、高危 finding 都会判红；
审计原件不删。

## 5. 本轮实测与修复

- `security_gate --mode quick` 在本仓第一次跑出 2 个 high 依赖漏洞：
  `libsqlite3-sys 0.37.0`（RUSTSEC-2026-0459）和 `rustix 1.1.5`
  （RUSTSEC-2026-0486）。
- 已把 `crates/trit-clean`、`crates/trit-orch` 的 `rusqlite` 从 `=0.39.0` 升到
  `=0.41.0`（`libsqlite3-sys 0.39.1`），把 `rustix` 升到 `1.1.6`；
  `cargo check -p trit-clean --locked` 与
  `cargo check -p trit-orch --features tritdb-sql --locked` 均通过。
- 复跑 `cargo audit --json`：**0 vulnerabilities**；剩余 2 条 medium
  unmaintained（`fxhash`、`rustls-pemfile`），不阻断 high 门，但保留在报告里。
- `quick` 复跑 verdict=pass，findings=2（均 medium），gaps=0。

## 7. MCP / 插件审核规则（不把供应链当免费午餐）

- 只接入来源可追溯的 MCP/插件：仓库、发布者、版本、命令 argv 四项都能写进配置；
  不认识的包不自动 `install`，不把 `npx -y <unknown>` 当默认。
- MCP 配置里只允许环境变量名，不写密钥字面量；`trit-procscan mcp <config> --strict`
  做 stdio 握手和上下文成本体检，失败/超时按运行时缺口记账。
- 第三方插件产生的 finding 不直接当成本仓结论：先由 `security_gate` 归一化
  severity 和路径，再由人决定是否加入 `security.commands`。
- 100+ LSP 注册表同理：注册表是候选池，`detect/plan` 先看本机有什么，
  `install --apply` 只执行 allowlist 里已审核的包管理器配方；不搞全量静默安装。

## 6. 边界（说清楚没做什么）

- 没有把 Semgrep/Trivy/CodeQL 全部内置成依赖：它们是可选 adapter，命令存在才跑。
- 动态扫描只对已有 fuzz target 做有界 fuzz（1000 runs），不是全量 libFuzzer 长跑；
  长跑在 `.github/workflows/quality.yml` 的 `fuzz smoke` 矩阵。显式 `--mode dynamic`
  或 `--mode all` 时，项目有 `fuzz/Cargo.toml` 但 `cargo-fuzz` 未安装会记**必需 gap**
  并 fail-closed，不伪装成"动态扫描通过"。Windows GNU 工具链可能无法构建
  sanitizer 目标，同样记 gap；真实动态门在 Ubuntu CI/WSL 上跑。
- 运行时扫描只做 MCP 配置 stdio 握手/上下文成本，不做应用进程的 ETW/内核级追踪；
  后者需要管理员权限，`trit-procscan` README 已写明采不到什么。
- **privacy 扫描需要 git 索引**（`git ls-files`）。非 git 树会把 privacy 记为
  **必需 gap** 并 fail-closed，不伪装成通过；用户可先 `git init`/`git add`，或在
  `.agent/settings.json` 里显式关掉 `privacy` 扫描器。
- 没有把 "unknown" 当 pass：必需扫描器缺口、未解析的扫描输出、超时都保留为
  gap 或 high finding。
