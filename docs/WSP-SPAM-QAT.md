# 三值 QAT 训练 + 工件红队：真实数据集全链（SMS Spam，2026-11-14）

**真实数据集 → 纯 Rust 训练 → 三值模型工件 → 红队读数**的第一次完整闭环。
回应的问题是：「数据集不能只是三进制（真实世界数据），模型本身是三进制，
缺的神经部件自造（训练禁 Python 硬门一致）——这样训出来的安全分类器，
工件被攻击者改写后会怎样？」

## 1. 管线（`examples/spam_redteam.rs`，391 行）

| 部件 | 实现 | 来源 |
|---|---|---|
| 数据 | UCI SMS Spam Collection（5,574 条 / 747 spam，`ham/spam<TAB>text`） | 下载直链，见 `docs/DATASETS.md` A0 |
| 特征化（自造） | 字符 3-gram/4-gram FNV 哈希到 4096 维，出现=+1 ⇒ **输入本身三值**（存在性编码），无预训练 embedding | 本 PR |
| 模型 | QAT 三值 MLP `4096→128→1`：`TernaryDense` + `Ste::Clipped{1.0}`，Tanh/Tanh，hinge | `trit-learn` 已有 |
| 训练循环 | 单样本 SGD、逐 epoch 确定性洗牌、Adam(latent)+Adam(scale)、**行 absmean 初始 scale** | 本 PR（scale 初始化是关键，见 §3） |
| 评测（自造） | 分层 70/15/15（确定性洗牌）、acc/P/R/F1（spam=正类）、混淆矩阵 | 本 PR |
| 导出 | 量化权重 + scale → `trit-wspace` `Layer::with_scales`（bias=0）→ WSP 工件，推理口径 `y = scale·(t·x)` + Tanh | 本 PR |
| 红队 | 比特翻转攻击（`Edit::SetTrit`+`Patch`），k ∈ {1e3, 1e4, 1e5, 5e5}，5 次重复取中位数，度量 spam F1 / 漏检率 | 接 #489 线 |

复算：

```bash
curl -o smsspam.zip https://archive.ics.uci.edu/static/public/228/sms+spam+collection.zip
cargo run --release -p trit-wspace --example spam_redteam -- \
  --file D:/KF/trit-data/sms-spam/SMSSpamCollection
```

## 2. 读数（seed=7/11，30 epochs，单线程 release）

| 项 | 值 |
|---|---:|
| 训练 / 验证 / 测试 | 3,900 / 836 / 838 |
| val（epoch 30） | acc 0.9557 / F1 0.8213 |
| **QAT 测试** | **acc 0.9403 / F1 0.7500（P 0.862 / R 0.664）** |
| **WSP 工件测试** | **acc 0.9403 / F1 0.7500 —— 与 QAT 逐位一致** |
| 参数（trits） | 524,416 |

工件导出无损（量化推理 = QAT 前向），验证了 `trit-learn` 的量化口径与
`trit-wspace` 的 scale 语义在同一条线上。

## 3. 过程中的两个坑（对后续训练有用）

1. **scale 初始化决定生死**：latent=±0.05、scale0=1 ⇒ `round(w/scale)=0`
   全层量化成 0，网络是死的（loss 恒 1.0，全预测多数类）。scale0 必须取
   **行 absmean**（与 `Layer::from_f32` 同口径），第一个 epoch 就正常收敛。
2. 特征化里 `len + 1 < width` 的 usize 下溢（release 静默 wrap）——已改
   `len < width`。

## 4. 红队读数：判别式分类器 vs 生成式 MLP 链

| k（翻转 trits） | F1 中位数 | spam 漏检率 |
|---:|---:|---:|
| 0（干净） | 0.750 | 33.6% |
| 1 000（0.19%） | **0.205** | 25.7% |
| 10 000 | 0.249 | 7.1% |
| 100 000 | 0.249 | 2.7% |
| 500 000（95%） | 0.238 | ~0（全判 spam） |

与 `docs/WSP-REDTEAM-MODEL.md`（Pythia-70M 生成式 MLP 链，k=10⁴ 余弦仍
0.9999）**方向相反**：

- **判别式安全分类器对比特翻转极度脆弱**：0.19% 的 trit 翻转把 F1 打掉 73%。
  机制：hinge+tanh 的决策边界由少数高 |scale| 行承载，而输入是稀疏存在性
  特征——少量高承重行的翻转足以平移边界；
- **攻击后模型退化成「常量预测器」**：k 大之后漏检率单调下降到 0（全判
  spam）——攻击把网络推向单侧输出，而不是精细误分类；
- **结论**：三值工件若用于安全判别（垃圾/钓鱼/恶意），完整性保护不是
  可选项——容器校验和（FNV，任意 1 bit 检出）必须配签名/审计链，且
  「检出后拒绝加载」应成为加载器默认行为。

## 5. 边界（未证栏）

- 单数据集单模型；hinge+tanh 组合之外（sigmoid+BCE、多类 softmax）未调；
- 30 epochs 未收敛到平台（val F1 还在涨），更多 epoch/学习率调度未扫；
- 「行方差攻击」这类白盒定位在本分类器上未做（#489 的读数显示小 k 时
  白盒增益 2.6-5.7×——本模型的脆弱性比那更强，随机攻击已足够）；
- 特征是字符 n-gram 存在性，无词序/语义特征；与 f32 浮点基线（同结构
  非 QAT）的对照未做——本读数只回答「三值工件被改写会怎样」，不回答
  「三值化本身损失多少」（cifar_tasks 与 #447 已有类似口径）。
