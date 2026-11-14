# 文档索引规则：新增 / 搬迁 / 改名

> 定位：任何要往 `docs/` 加文件、挪目录、改文件名的人。它规定**流程与判红条件**，
> 目的是让「加了文档但没人找得到」这件事不再靠记性。
> 骨架见 [DOCUMENTATION.md](DOCUMENTATION.md)，入口在 [docs/README.md](../README.md)。

## 1. 目录与分组

| 目录 | 判据（满足即放这里） |
|---|---|
| `positioning/` | 回答「这个库是什么 / 不是什么 / 怎么归位 / 到哪一步」 |
| `reference/` | 回答「某个组件或格式长什么样」——可当手册查阅，不随实验变化 |
| `results/` | 有可复算命令与读数的结论（含负结果） |
| `research/` | 研究线过程：方法、读数、未证栏；结论尚未进稳定 API |
| `security/` | 威胁模型、红队/防御实验、并发与高危政策 |
| `engineering/` | 构建、worktree、更新、运维 |
| `plans/` | 还没落地的计划；落地后挪进 `results/` 或写进 crate 文档 |
| `specs/` | 写作与治理规范本身 |
| `review/` | 按日期的评审快照（历史，不改写） |

落不进任一目录的文档，通常说明它该写进 **crate 文档**（`crates/*/README.md` 或 rustdoc），
而不是 `docs/`。

## 2. 新增一篇文档（三步，缺一步等于没提交）

1. **写文件**：按 [DOCUMENTATION.md](DOCUMENTATION.md) 的四段骨架，文件名大写英文 + 连字符。
2. **进索引**：在 [docs/README.md](../README.md) 对应分组的表里加一行（`文件名` + 一句话）。
3. **建入链**：让**至少一篇别的文档**指向它——最自然的位置是所属分组之外的入口
   （`positioning/` 或某篇 `results/` 的「相关文档」）。

第 3 步是过去最常漏的一步：一度有 25 篇文档除了 `CHANGELOG.md` 之外没有任何入链，
读者只能靠文件名猜内容。

## 3. 搬迁或改名（四步）

1. `git mv` 移动文件（保留历史）。
2. **改全仓引用**：`docs/` 内部、根 `README.md`、`crates/*/README.md`、`AGENTS.md`、
   `CONTRIBUTING.md` 里的相对链接与 `docs/` 路径。迁移后目录深度会变，
   `../x.md` 可能要变 `../../x.md`，必须逐条按新位置重算。
3. **改脚本里的路径**：`scripts/` 下门脚本的扫描面与默认输出路径、`scripts/*_baseline.json`
   的键、`sweeps/*.json` 里的路径。漏一处，门就悄悄扫不到东西（比扫到错误更危险）。
4. **再跑一遍门**（见 §4）。

`CHANGELOG.md` 与 `review/` 是历史记录：只把**可点击链接**改到新路径，正文里的旧路径描述保留。

## 4. 判红条件与命令

```bash
python scripts/doc_pointers.py     # 行号指针指得对 + markdown 链接可达 + 每篇文档在索引里
python scripts/docs_hygiene.py     # CHANGELOG 形状 + 盘符路径不新增
python scripts/claims_evidence.py  # 能力句有没有读数/复算指针（棘轮，只许变少）
python scripts/model_status.py     # STATUS.md 的表与代码对得上
```

判红情形：

- 新文档不在 `docs/README.md` 的表里 ⇒ `doc_pointers.py` 报「未索引」。
- 链接指向已搬迁的旧路径 ⇒ `doc_pointers.py` 报「链接不可达」。
- 文档里写了本机盘符路径 ⇒ `docs_hygiene.py` 报「盘符路径新增」。
- 能力句没有读数或指针 ⇒ `claims_evidence.py` 报「无据能力句，基线允许 0」。

## 5. 现行落地程度（别把规则当成已经有门）

| 规则 | 落地 |
|---|---|
| 行号指针、链接可达、索引覆盖 | `门` `scripts/doc_pointers.py` |
| CHANGELOG 形状、盘符路径 | `门` `scripts/docs_hygiene.py` |
| 能力句带读数 | `门` `scripts/claims_evidence.py`（棘轮：新文档按 0 计） |
| 分层与依赖方向 | `门` `scripts/layers.py`（跨层例外登记在 `scripts/layers.json` 的 `allows`） |
| 交接账本编号唯一 | `门`（**本机门**）`scripts/tech_collide.py`——它读仓外交接账本
  `_handoff/LEDGER.md`，CI runner 上没有那份账本，所以**不在 CI 里跑**；本地判定命令给在
  上一行。宁可写「本机门」，也不加一个在 CI 上永远 skip 的步骤假装覆盖 |
| 分组判据、入链、命名 | `人` 评审——没有门，也没打算为风格造检查器 |

## 6. 未证栏

- 「入链」只要求「有别的文档指向它」，没有检查**指向是否合理**（可能只是 CHANGELOG 里提一句）。
  真要判质量只能靠评审。
- 索引覆盖检查基于文件名解析；同名文件（不同目录）会要求写全相对路径，否则可能误判可达。
