# README 规范

> 定位：改根 `README.md`、crate `README.md`、或 GitHub 仓库简介的人。
> 目标是止住一种反复：首页承诺一套，文档正文另一套，读者不知道信哪个。
> 骨架与硬规则见 [DOCUMENTATION.md](DOCUMENTATION.md)，本文件只管**首页特有的要求**。

## 1. 首屏只放三件事

`README.md` 的前三段（读者不滚动就能看到的范围）只允许出现：

1. **是什么** —— 一句话定位，与 [DESIGN.md](../positioning/DESIGN.md) §1 的定位句**同一口径**，
   不许另起说法。
2. **不是什么** —— 一句显式排除（例：不做 EDA 全流程、不做通用模型检验器、稳定 API 面不含
   平衡三进制整数类型）。
3. **怎么验证** —— 指向可复算入口（命令或结果文档），不是形容词。

「工具箱 / 底座 / 平台 / 框架」这类名词只能选一个，且必须与 [LAYERS.md](../positioning/LAYERS.md)
开头那句话一致。

## 2. 定位句的单一真源

- 定位句的**正本**是 [DESIGN.md](../positioning/DESIGN.md) §1（一句话）与
  [LAYERS.md](../positioning/LAYERS.md) 开头（底座、不是工具箱）。
- 首页、GitHub 仓库简介、`crates/sansheng/Cargo.toml` 的 `description`、`AGENTS.md` 第一段，
  都必须从这里取，不许各自发明。
- 发现两处不一致时：**先改正文正本，再同步四处引用**；不要为了让首页好看而改正本。

## 3. 能力必须标层

- 工作区表格里每个 crate 都要能看出是**稳定 API** 还是**实验层**（`publish = false`）。
  实验层单独成表，表头写明「不属于稳定 API」。
- 实验层的数量与判定**以 `Cargo.toml` 为准**，不靠人记：
  `grep -l "publish = false" crates/*/Cargo.toml | wc -l`。
  写数字时给这条命令（[DOCUMENTATION.md](DOCUMENTATION.md) 规则 1）。
- 一个能力没在 [STATUS.md](../positioning/STATUS.md) 登记时，首页不列它。

## 4. 性能与精度数字

- 每个数字必须同时给：**同机对照**（f32 / f16 / Q4_0 / 1-bit 等）、**命令**、**规模档**
  （例如 512×4096×512）、**图表或结果文档的入口**。
- 不同机器的数字分列，不做加权平均，不跨机比较后下结论。
- 只引用本仓可复算的读数；外部论文/博客数字要标来源并写「未复现」
  （[TERNARY-BOUNDARIES.md](../positioning/TERNARY-BOUNDARIES.md) 硬规则 6）。

## 5. 结构约束

- 每节控制在「一屏」（约 40 行为宜），长清单挪进 `docs/` 并在首页留一行指针。
- 首页不复制 `docs/` 正文；指针要指向**当前路径**（搬迁后同步，见 [DOC-INDEX.md](DOC-INDEX.md)）。
- 图表放 `docs/assets/`，由 `scripts/gen_docs_charts.py` 生成，首页只引用不手改 SVG。
- 不写本机盘符路径、用户名、临时目录（`python scripts/docs_hygiene.py` 会判红）。

## 6. 改动首页时的检查清单

```bash
python scripts/docs_hygiene.py        # 形状 + 盘符路径
python scripts/claims_evidence.py     # 首页的能力句有没有读数/指针
python scripts/doc_pointers.py        # 行号指针与链接可达
```

## 7. 未证栏

- 「首屏三段」「每节一屏」没有机器检查，靠评审；发生过首页膨胀后再靠人删，代价比预防高。
- 首页数字与 `docs/results/` 的同步没有门：结果是自动生成的，首页是手写的，两边可能漂移。
  目前靠「首页只给一个代表数字 + 指针」来限制漂移面。
