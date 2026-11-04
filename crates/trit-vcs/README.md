# trit-vcs

内容寻址的协作版本库：BLAKE3 逐块寻址 + 内容定义分块 + 打包压缩 + 哈希链事件日志，
外加 Git 没有的那一面 —— **写之前先判**（认领门）、**判了就把解锁命令一起给你**、**每次 ref 移动都进哈希链**。

定位是"替代 Git 的候选"，不是 Git 的兼容层。`Cargo.toml` 里 `publish = false` 且描述写明
**不属于稳定 API**：接口还在动，别按发布库依赖它。

## 逐维度的账

下表每个读数都能用本节末尾的命令复算。同机对照 `git`，除注明外均为 `--release`。
**没有读数的维度一律不写"更强"。**

| 维度 | 结论 | 读数 |
|---|---|---|
| 对象存储体积 | **认输** | 三形状 A `1.06×` / B（共享块）`2.29×` / C `1.02×`（×>1 就是比 git 大）；第一版每块一文件时 4.87×，靠打包+zlib 才追到现在这个位置 |
| 大仓 status | 更快 | 10k 文件热改动 **24–32 ms** vs git 34–35 ms（**0.7–0.9×**）；靠 `(size, mtime_ns)` 内容缓存 + 范围内 status |
| 部分拉取 | 更强 | 200 文件 / 20 目录：全量 publish **162,854 B / 402 对象**，单目录 pull **8,226 B ⇒ 5.05%**；重复 pull 实传 **0 B**（22 个对象全跳过） |
| 传输断点 | 更强 | 截断的包再导入续传，结果**逐字节相同**；导入跳过目标已有对象 |
| 合并产物 | 更强 | 300 行单点争用：结构化冲突对象 **381 B** vs 两侧全文 7,780 B ⇒ **4.90%**；只差行尾空白的"伪冲突"**0** 条 |
| 写前准入 | Git 无对应面 | 未认领的提交默认拒；认领有 TTL、同名 id 判红；判决是三态（True/Unknown/False），"不知道"不等于"过" |
| 被拒之后 | 更强 | 每条判决附**能直接跑的下一步命令**：`unclaimed` 负载 329→455 元（3 条命令）、`claim_conflict` 18→88 元、`witness_required` 26→73 元 |
| 预检成本 | 有读数 | 60 文件 / 121 对象 / 9,400 字节的预检 **4.8 ms**，**写盘 0 个对象**（对象数、哈希链长度、HEAD 三样分别断言） |
| 审计 | 有面 | 每次 `set_ref` 追加进哈希链（改分支不留痕这条走不通）；提交可带 `evidence`/`tags`，`--tag/--evidence/--grep` 可查 |
| 时间 | 有面 | HLC（混合逻辑时钟）而非裸墙钟 |
| 上下文预算 | 有面 | `--budget <元>` 对所有读数生效：只丢整行、从尾部丢，`truncation` 元数据自己算进预算；ASCII 4 字 = 1 元、非 ASCII 4 元 |
| Git 生态 | 薄 | 双向桥只走第一父链：`git-export` / `git-import`（同步删除、防路径穿越、剥 `Trit-*` 尾注）；无 merge 图 / submodule / tag |

## 明确没做的（别当成已具备）

- **没有网络协议，也没有鉴权**：`publish` / `pull` 的"中央仓"就是**另一个目录**。跨机、认证、多租户一概没有。
- `pull` 不动客户端 HEAD，也不删本地多余文件（它只补范围内的东西）。
- 认领账本是**跨仓**的账，不由本 crate 持有；`--repo <owner/repo>` 在给出的命令里仍是占位符。
- v0 的 witness **不签名** ⇒ 防篡改、防绑错树，不防有人自己造一份。
- 大仓 status 只做了 stat 缓存这一层；没有 sparse-index 那种持久索引。
- 三值逻辑只用在裁决处：全仓 `src` 实测三值 **4.5%**、二值 2.2%、中性 93.4%（`scripts/parity_scan.py`）。

## 复算

```sh
cargo run --release -p trit-vcs --example vcs_bench -- --central   # 存储 / status / 传输 / 合并 / 中央仓
cargo run --release -p trit-vcs --example agent_ergonomics         # 被拒负载与预检读数
cargo test -p trit-vcs                                             # 12 个测试二进制 / 141 条（2026-11-05 实测）
python scripts/mutation_check.py --pkg trit-vcs                    # 96 条：95 断言抓住 + 1 条声明的死循环
```

最后一条要在单体仓 `bfxh/trit` 里跑（`scripts/` 还没随拆分搬走）。变异登记的口径：
每条 = 一个单点文本改动 + 点名的那条测试；测试必须变红，否则算存活。

## 库面

| 模块 | 一句话 |
|---|---|
| `store` / `chunk` / `hash` | 内容寻址的对象库、CDC 分块、BLAKE3 id |
| `bundle` | `TVBD` 包 + 过滤后的**树清单** + 范围内传输（export 与 pull 共用同一条路） |
| `central` | 命名 ref（`.tritvcs/refs/<name>`）、`publish`（闭包 + 父链 + 只准往前推）、`pull`（稀疏物化，脏则拒） |
| `repo` / `journal` | 提交与查询、哈希链事件日志 |
| `preflight` | `commit()` 与 `preflight()` **共用**的唯一判决处 ⇒ 预检不会和真提交分叉 |
| `remediation` | 每条 `RepoError` → `kind` + 可执行命令（`--json` 下失败也是纯对象） |
| `tokens` | 一把 token 尺，CLI 与 MCP 共用 |
| `gate` / `witness` / `model` | 认领门（三态判决 + `ClaimSpec`）、提交见证（in-toto 形状，v0 不签名）、`Commit`/`HLC`/`Intent` |
| `worktree` / `merge` / `delta` | 扫描与稀疏物化、行级三路合并、前后缀 delta |
| `git_bridge` / `techmap` / `status_cache` / `mcp` | Git 双向桥、技术标签索引、status 缓存、13 个工具的 stdio JSON-RPC |

## 踩过的坑（留在仓里，免得再踩）

- 部分拉取曾按"整棵树"发货 ⇒ 单目录测出 49%，看着像实现错其实是判据没对齐；抽出 `bundle::manifest` 后
  export 与 pull 不可能分叉。**两处同源的坑要合并成一处，而不是各修一遍。**
- `--budget` 曾经"压进预算"是假的：丢掉 `truncation` 元数据自己占的额度才能压进去 ⇒ 现在元数据算进预算，
  且有一条"预算正好等于原读数就必须真丢一行"的判据钉住。
- 认领闸门与变异门两个后台任务在同一台机器上互抢 target 目录锁会一起挂死 ⇒ 门改成单实例 + 每条超时 +
  原子还原；超时的条目**判红而不是算抓住**。
- 拆掉"数组空了就 break"的守卫后测试永不返回，这类变异**没有断言能钉住** ⇒ 登记表里显式声明 `hang_secs`，
  单列 `HANG-KILL`，不计入"断言抓住"。
