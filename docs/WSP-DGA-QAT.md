# 三值 QAT：DGA 域名检测与家族归因（真实安全数据集，2026-11-14）

接 `docs/WSP-SPAM-QAT.md` 的全链，换**更大的安全数据集**：DGA（算法生成域名）
检测 + **家族归因**。白客口径：检测回答「是不是恶意」，归因回答「哪个家族」——
后者是应急响应（who/what to block）的输入，也是多分类评测部件的用武之地。

## 1. 数据

`chrmor/DGA_domains_dataset`（GitHub 直链，研究许可，引用
Cucchiarelli et al. 2021）：Netlab 360 的 **25 个 DGA 家族**（每家族 13,500）+
Alexa benign（337,398），共 674,898 条，CSV `label,family,domain`。
下载与登记见 `docs/DATASETS.md` A0；落数据盘 `trit-data/dga/`。

## 2. 管线（`examples/dga_redteam/`，features + main 两个文件）

- 特征（自造）：域名 **2/3-gram** FNV 哈希 2048 维，出现=+1——域名短、字符
  集小（`a-z0-9-.`），存在性三值输入比 spam 的 3/4-gram 更贴合；
- 模型：QAT `TernaryDense` 2048→H→C（`Ste::Clipped`，Tanh 隐层，softmax
  交叉熵——`trit-learn` 现成部件），H/lr/epochs 全部 CLI 可调；
- 评测（新部件）：**`trit_learn::metrics::Confusion`**（多类混淆矩阵 +
  acc + per-class P/R/F1 + macro-F1）下沉到可发布层（3 条单元测试），
  这是本 PR 新造的神经评测部件；
- 红队：比特翻转（`Edit::SetTrit`）后的归因准确率。

## 3. 读数（seed=7，release 单线程）

### 3.1 二分类（dga vs legit，per-class 3000，hidden 64，20 epochs）

| 项 | 值 |
|---|---:|
| 测试（QAT 与 WSP 工件**一致**） | acc **0.778** / macro-F1 0.778 |
| 红队 k=1 000（0.76% trits） | acc 0.778 → **0.641** |
| 红队 k=10 000 | acc **0.444** |

### 3.2 多分类归因（26 类）——两轮都未达标，如实记录

| 配置 | 结果 |
|---|---|
| 800/类，hidden 64，lr 1e-3，24 epochs | acc 0.294 / macro-F1 0.243（loss 2.90→2.73 缓降） |
| 1 200/类，hidden 128，lr 3e-3，60 epochs | acc **0.266** / macro-F1 0.217（**loss 回升 3.77，lr 过大发散**） |

**家族归因是细粒度任务，M0 单线程 QAT 管线在容量与训练配置上都没到位**
（文献同数据 n-gram + 大 MLP 可达 90%+）。负结果两条：lr 3e-3 在此规模
发散；per-class 样本量从 800→1 200 无改善。归因达标需要：更小 lr + 更长
训练（或 mini-batch/学习率调度部件——均未造）。二分类 0.778 已证明特征
与管线本身可用。

### 3.3 红队判读（与 #489/#492 汇总）

| 任务 | trits | 0.76% 翻转的影响 |
|---|---:|---|
| Pythia MLP 链（生成式，#489） | 12.6M | 余弦 0.9999（无感） |
| SMS spam 分类器（判别式，#492） | 524k | F1 -73% |
| DGA 检测（判别式，本 PR） | 131k | acc -14pp（k=1 000） |

**规律成型**：判别式小工件（参数少、决策边界由少数高 scale 行承载）对比特
翻转的脆弱性 >> 生成式大链；工件越小越需要完整性保护。三档任务一致支持
「三值安全工件 = 校验和 + 签名 + 检出拒绝加载」的部署底线。

## 4. 未证栏

- 26 类归因未收敛（见 §3.2）；文献（同数据集 n-gram + 大 MLP）可达 90%+，
  差距在容量/训练时长/特征宽度，不在特征本身（二分类 0.778 已证明特征
  信号可用）；
- 白盒定攻击（#489 的行方差 / analyze 实测）未在本任务展开；
- 未与 f32 非 QAT 对照；家族混淆矩阵的细读（哪些家族互相混淆）只落了
  JSON，未入册。
