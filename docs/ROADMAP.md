# 主库路线图（该搬进 / 该新开 / 该搬出）

> 只写**底座自己的事**。项目口径（数据协议、产品判据、上线手册）留在各生产链仓，
> 这里最多留一行指针——复制过来就会漂移，这是本仓踩过的坑（镜像副本判定相反那次）。
> 判据正本在 [LAYERS.md](LAYERS.md)（归位）与 [SCOPE.md](SCOPE.md)（收 / 不收），本文件的每一行都要能对上它们。

## 1. 该搬进来的（现在散在生产链仓，按 §1 属于 base/ops/algo/learn）

| 外面的东西 | 进哪层 | 只搬哪一半 | 现状证据 |
|---|---|---|---|
| diffusion 的三值模型头与一步生成算子 | learn + ops | 模型与算子；训练脚本、数据管道留项目 | FID 43.1 / 全三值 45.1 / 2-bit 14.3× |
| lacbs 的三值扩展（`reject_hopeless`、SAT 侧改造） | algo | 判据与求解件；产品面留 lacbs | 现按 `trit-core = "0.3.2"` 版本号反向依赖主库 |
| memtri 的三值检索打分内核 | ops | 打分内核；MCP 外壳与集成留 memtri | 命中 98.5% |
| vxl-phys 的三值裁决面 | algo | 语义与判定表；物理内核留 BSHSQ | M0 交付 #300，M1 步 0 排序 3.8%/7.0% |
| **评测协议三件**（清单 + 内容指纹 + 独立复核器） | learn，从 `trit-parts` 下沉成 `trit-learn::manifest` / `::verify` | 三件全部 | depth / parts 两族已各写一遍 SHA-256 与复核器 |

判据一句话：**"移植别的算法、只是用三值/四值实现"的进来；"把主库接到我某个产品上"的留外面。**

## 2. 该新开的（主库真缺，且能被现有件对拍）

| 新库 | 层 | 为什么现在就该做 | 对拍谁 |
|---|---|---|---|
| ~~`trit-bench`~~ **已建**（2026-11-05） | base | 交错 ≥6 轮、min/median/max、规模档表现在**至少 5 份手写实现**（trit-dot main、trit-ops bench、trit-gpu bench、trit-parts parts_bench、`benchmarks/dot_variants`）；这是 §4 第一批要登记的重复能力 | 与 `benchmarks/dot_variants` 的读数对齐 |
| ~~`trit-fuzz`~~ **已建（第一批）**（2026-11-05） | base | 精确尺 `grep -rlE "\^=.*<< 13" --include=*.rs crates/` ⇒ 本会话前 **67** 个文件各自手写 xorshift；`trit-fuzz` 第一批后 **65**、`trit-dot`/`trit-codec` 的 stress 改吃后 **63**、再吃下 `trit-float`/`trit-immune` 三份测试后 **60**（口径 `crates/`；含 `benchmarks/` 再多 1）（第一版我用宽松尺数到 64——散文里提到的模式也被算了，换成要求同一行有 `^=` 的精确尺）。收了：三值向量 / 表达式 / 变量名 / **CNF 子句集**（DIMACS 口径，无重字、无永真、无空子句）/ 单值取样 `trit`·`ternary_value`·`bipolar` | 消费者五条：`trit-count/tests/cnf_fuzz.rs`（432 例 DPLL vs `2^n` 穷举；`dpll.rs:78` 改错能抓 96 个不一致）+ CLI `trit-count --fuzz-cnf`（规模档地板：`--nvars 18/20/22` 最慢一题 183–294µs、24 档 490–538µs，各 3 次同种子同语料，全部与穷举一致）、`trit-dot/tests/stress.rs`、`trit-codec/tests/stress.rs`、`trit-float/tests/stress.rs`、`trit-immune/tests/fuzz_smoke.rs` |
| `trit-ir` | algo | `.tri`/TTIR 只有"化简 + 打包"，没有 **pass 框架**（常量折叠、等价重写、CSE） | `trit-sat` / `trit-bdd` / `trit-count` 当每个 pass 的 oracle |
| ~~`trit-mv`~~ **已建（第一批）**（2026-11-11） | base | 有限 Łukasiewicz 链（整数精确）+ 4 种 t-norm（Minimum/Łukasiewicz/Drastic/幂零最小）+ residuum；三值档与 `trit-core` Kleene 逐点对拍 | 链上公理穷举（交换/结合/单位/单调）+ 伴随律（左连续三种）+ Kleene 对拍 |
| ~~`trit-num`~~ **已建（第一批）**（2026-11-12） | ops | Neumaier 补偿求和、Welford 流式统计（可合并）、最小二乘/岭回归（正规方程 + Cholesky，截距不惩罚）、中位数/MAD、均值/中位数/线性插补；附 **f32 参考实现**对拍 | 精确直线恢复（<1e-12）、f64 vs f32 同题偏差（<1e-4）、奇异/共线/非有限输入结构化报错 |
| `trit-export` | ops | 三值权重容器（量化 GGUF）还没有落点，属 codec/learn 边界 | 与 `trit-codec` 打包逐位一致 |
| ~~`trit-belnap` 的算子面~~ **已建**（2026-11-11） | ops | 真序/知识序双格（共识 `⊗`、轻信 `⊕`、共轭 `¬ₖ`）+ 多源合并（真序 join / 知识序 consensus）；**接入 trit-clean 的合并策略仍是后续** | 双格公理穷举（分配/吸收/结合）+ 已知序关系 + De Morgan（真序 `¬`、知识序 `¬ₖ`） |
| `trit-clean`（**后期，不排期**） | algo | 三值约束检查 + 精确修复计数 + 三支决策（收/拒/转人工）；方向评估与底层缺口见 [DATA-CLEANING-PLAN.md](DATA-CLEANING-PLAN.md) | 与 SQL 三值语义、小规模穷举计数对拍；BDD×DPLL 交叉校验 |
| `trit-vault`（**后期，不排期**） | ops | 照片/视频/文本的静态隐私：盘上只有随机噪声、元数据全加密，经自有挂载器（WinFsp/FUSE）正常打开；威胁模型与硬边界见 [PRIVACY-PLAN.md](PRIVACY-PLAN.md) | 容器/清单口径与 `trit-codec` 对拍；挂载器验收按计划 P2/P3 判据 |

**挡住别人的一条**：`trit-mv` 不做，"用四值/n 值移植算法"就没有落点，只能各项目自己搓。

## 3. 该搬出的（主库的 prod 层太重）

`trit-orch`、`trit-procscan`、`trit-redteam`、`trit-unsafe`、`trit-lsp` 都是外壳，不是底座
（prod 现在 12/35）。方向是拆 `bfxh/trit-tools`，能同时瘦 CI 面与可发布面。**顺序硬要求**：
分层门先进 CI、#289 合完，再谈拆分——两边同时动 `.github` 一定撞车。

## 4. 排序

- **本周**：~~`trit-bench` 立项吃掉那 5 份重复 bench~~ **已做**（PR #299；`layers.json` 的 `capabilities`
  登记了 `bench-timing-harness`，两份未迁的手写档各带实测差与保留理由——"并存"这条已按口径办）。
  评测协议从 `trit-parts` 下沉 `trit-learn`；~~watched literals~~ → **残基子句表扁平化 / 缓冲复用**
  （靶子在 2026-11-06 被自己的探针改了：匹配族 **allocs/节点 164.1 → 925.5（5.6×）**，同档 **µs/节点 10.8 → 52.8（5.0×，
  5 次重复中位数、机器空转）**，两条线同步线性上升；按自标定单价 **35.4 ns/次 alloc+free**（含计数开销，是上界）折算，
  分配能解释 **≤64%** 的上升 ⇒ 省扫描次数的监视器不是这一刀的瓶颈，`dpll.rs` 头部那句"瓶颈是小分配"只解决了一半：
  CSR 出现表扁平化之后，**残基子句表仍是每条子句一个 `Vec`**。读数出处
  `crates/trit-count/examples/matching_family.rs` 与 `crates/trit-count/examples/matching_alloc_probe.rs`。）**这一刀当天已落地**：`dpll.rs` 换成扁平 `Table` 后 allocs/节点 164.1/925.5 → **37.3/41.3**（曲线变平），40 对 **1.92×**、20 对 **1.73×**，四列计数逐位不变。
  **已被消融否证的路线**：省 `decompose`/拷贝不是下一刀——去掉分解后 12 对从 4.7 ms/323 节点涨到
  1807 ms/729,059 节点（×384），分解的开销跟它省下的搜索不在同一量级。
- **下月**：`trit-mv` → `trit-ir` → `trit-num`。
- **择期**：`trit-tools` 拆分；改名（顺序见 LAYERS §6，外部 5 面证据已列全）。

## 5. publish 面（可发布性口径）

- 现状：**19 个 crate 仍 `publish = false`**（实验层），16 个可发布。可发布面是承诺：签名进 semver，
  动它要走「先 `deprecated` 一个 tag，下个 tag 才删」。
- 本仓的硬约束（`publish_graph.py` 判红）：可发布 crate **不得**依赖 `publish = false` 的 crate。
  这条就是 `trit-cli count` 之前的堵点。
- **本轮翻面：`trit-count` → 可发布**（名字在 crates.io 实测未占用：`GET /api/v1/crates/trit-count` = 404）。
  判据不是"它成熟了"这种虚话，而是三条具体的：① 计数有**三方独立对拍**（DPLL / 穷举 / 闭式解 / BDD）；
  ② 有 26 条测试 + 16 条变异全抓；③ 它依赖的 `trit-core` / `trit-eval` / `trit-bdd` 本来就可发布，
  翻面不引入新的约束。 ⇒ `trit-cli` 的 `count models` 子命令已接上（PR #299）：
  `crates/trit-cli/tests/count_cmd.rs` 5 条二进制级断言，读数写在 README「模型计数」一节。
- **翻面 ≠ 发布**。真发 crates.io 是单独一次授权动作（登录 ≠ 授权、邮箱验证、新包限流、token 不落聊天记录），
  要发的时候单独说，我按 `scripts/publish.sh` 口径走。

## 6. 文档归位（谁写在哪）

- **主库 `docs/`**：语义、分层与归位、门、格式规格、跨引擎实测读数。
- **生产链项目自己的仓**：该项目的数据协议、指标口径、上线与回滚手册。
- 主库**不复制**项目文档正文，只留一行指针（`对象：bfxh/xxx · 文件：docs/yyy.md`）。
- 反向禁令：项目仓不要把主库的分层规范抄一份过去——抄了就会漂，指过来就行。
