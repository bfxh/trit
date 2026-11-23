# 三进制专项评估：编码可逆、逐位一致与有损量化（2026-11-23）

> 定位：对 `trit-codec` 的 10 类编码器、唯一有损的 `quantize`、以及 `trit-export` 的
> GGUF v3 容器做一次**独立复核**——不是「编码器再解一遍能对上」，而是另写最小实现与库输出
> 逐位比对。谁该读：改 codec / 容器 / 量化的人、准备引用「编码可逆」「量化误差有界」的人。
>
> 读它解决的问题：仓里多处写着「大多数编码器是无损的」「GGUF 往返与 trit-codec 逐位一致」
> 「量化误差小」，这些句子此前只有零散单测支撑，没有一份可一次跑完、失败即报红的对账。

## 边界

- **审可逆与位级一致**：往返、独立 oracle、纠错/检错的最基本不变式、非法输入拒绝。
- **审有损量化的误差界**：`quantize` 不能按往返审，改为审「逐元素误差上界」与
  「同比特预算下的重建误差读数」，并明确归入 `[B]`，不冒充 `[A]`。
- **审黄金语料的消费者**：`--corpus` 读 `ternary_corpus` 产出的 CSV，用评估工具里的
  独立代算复核；它证明语料能被消费，不等于第三方语言实现已经跑过。
- **不审性能**：不给吞吐、不给压缩率优劣结论；要谈快慢必须另行同机二进制基线。
- **不审训练效果**：量化后的模型精度、收敛、下游任务收益不在本次范围。
- **不审硬件 / 器件**：本仓不做原生三值器件仿真，本评估也不包含任何「原生红利」声明。
- **不替代**：单测、`tests/stress.rs`、`trit-fuzz` 各自覆盖的面；本评估只做它们没做完的交叉对账。

## 1. 口径：什么叫「独立复核」

三条判据，缺一条就不算复核：

1. **往返**：`encode → decode` 逐位还原（含空输入与长度边界）。
2. **独立 oracle**：在评估工具里另写一份最小实现，与库输出**逐位比对**——
   位级打包（`byte = Σ trit[i]·3^i`）、平衡三进制文本解析、mod 3 差分还原、稀疏 COO 重建、
   三值真值表代算。这条防的是「编码器与解码器同错，往返照样自洽」。
3. **不变式与拒绝**：Hamming 单符号错必须纠正并报出位置；CRC-3 单符号错必须检出；
   非法字节 / 越界索引 / 坏块长 / 零游程必须显式报错，不许猜。

独立 oracle 与库共享的是**线格式契约**（`trit-core` 的 pack 模块文档），不是实现代码。

量化另立口径：**误差界**是库不变式（可证），**重建 MSE** 是经验读数（只对这批语料成立）。

## 2. 语料与种子

- 形态 9 种：全 False / 全 True / 全 Unknown / 0-1 交替 / 0-X 交替 / 1-X 交替 / 稀疏（3% 有定义）/
  成段（每 5 个一段）/ 均匀随机。
- 长度 25 档：0、1、2、3、4、5、6、7、9、10、11、12、24、25、26、49、50、51、127、128、129、
  255、256、257、1024（覆盖 5 trits/byte 的字节边界）。
- 合计 225 个样本 / 22,239 个 trit；种子 `0x9e3779b97f4a7c15`，混入形态号与长度，**同种子逐位可复现**。
- GGUF 侧：1..6 行 × 1..20 列的全形状矩阵、全 Unknown / 全 True / 全 False 病态张量、
  33×129 大张量，以及 scale 的位模式敏感值（`-0.0`、最小正规、最小次正规、NaN）。
- 量化侧：6 种分布（均匀 / 近似正态 / 稀疏 3% / 离群 / 全正 / 全零）× 4 个长度
  （32、257、1024、4096）× 4 个 beta（0.3、0.5、0.7、0.9），共 96 个数据集 / 129,816 个元素；
  同一 seed 派生，**同种子同读数**。
- 黄金语料：`ternary_corpus` 产出 `truth.csv`（171 行，3 代数 × 57 组合）与
  `balanced.csv`（85 行，边界 + 进位密集 + 64 个随机 i64）。

## 3. 读数

`codec_verify` 已跑通：`cargo run -q -p trit-codec --bin codec_verify -- --json`，
**17 项检查 / 58,323 例 / 0 失败**（语料 225 样本 / 22,239 trit，seed `0x9e3779b97f4a7c15`）。

| check | cases | failed |
|---|---:|---:|
| pack.roundtrip | 675 | 0 |
| pack.independent_oracle | 5,427 | 0 |
| pack.rejects_illegal | 18 | 0 |
| balanced.roundtrip | 189 | 0 |
| balanced.independent_oracle | 226 | 0 |
| hamming.single_symbol_error | 90 | 0 |
| interleave.roundtrip | 138 | 0 |
| rle.roundtrip | 451 | 0 |
| rle.metrics_consistent | 450 | 0 |
| delta.roundtrip | 225 | 0 |
| delta.independent_oracle | 450 | 0 |
| gray.bijection | 9,844 | 0 |
| gray.adjacent_distance_one | 9,832 | 0 |
| sparse.roundtrip | 903 | 0 |
| sparse.independent_oracle | 225 | 0 |
| checksum.detects_single_symbol | 28,073 | 0 |
| plan.roundtrip | 1,107 | 0 |

`gguf_verify` 已跑通：`cargo run -q -p trit-export --bin gguf_verify -- --json`，
**4 项检查 / 237 例 / 0 失败**（seed `0x2545f4914f6cdd1d`）。

| check | cases | failed |
|---|---:|---:|
| gguf.roundtrip_all_shapes | 124 | 0 |
| gguf.metadata_and_determinism | 3 | 0 |
| gguf.rejects_bad_input | 6 | 0 |
| gguf.pack_bit_parity | 104 | 0 |

### 3.1 有损量化（新增）

| check | cases | failed |
|---|---:|---:|
| quantize.error_bound | 96 | 0 |
| quantize.idempotent | 96 | 0 |
| quantize.nan_policy | 1 | 0 |
| quantize.metrics_deterministic | 1 | 0 |

误差界（逐元素，beta ∈ (0,1)）：`|w − dequant(q(w))| ≤ scale·max(beta, 1−beta)`；
被判 `Unknown` 的元素另满足 `|w| ≤ beta·scale`。实测 `max(|err|/scale) = 0.899964`，
没有越过 beta=0.9 的理论上界。再量化幂等：`quantize(dequantize(quantize(w)))` 的 trit 序列不变，96/96。

重建读数（96 个数据集按元素数加权汇总，129,816 个元素）：

| 读数 | 值 |
|---|---:|
| 三元 MSE（双导轨 2 bit/trit） | 0.922640 |
| 1-bit 符号基线 MSE（1 bit/值） | 1,651.408428 |
| 2-bit 四电平基线 MSE（2 bit/值） | 183.252992 |
| 三元 / 2-bit 基线 MSE 比 | 0.005035 |
| 归一化 RMSE（RMSE/scale，按数据集平均） | 0.212914 |
| Unknown 占比 | 0.843702 |
| 三元 MSE / bit（双导轨 2 bit） | 0.461320 |
| 三元 MSE / bit（打包 1.6 bit） | 0.576650 |
| 2-bit 基线 MSE / bit（2 bit） | 91.626496 |

**不要把这组比值读成「三进制量化普遍更准」**：语料里有离群与稀疏分布，2-bit 均匀四电平
被离群 scale 拉坏，而三元有独立的 0 档，天然适合「大多数接近 0 + 少数大值」。
这是**数据相关**的读数，换分布可能反转，且 3 电平本身仍是 `[B]` 下的设计选择，不是 `[A]`。

### 3.2 黄金语料回灌（新增）

```bash
cargo run -q -p trit-codec --example ternary_corpus -- target/ternary-corpus
cargo run -q -p trit-codec --bin codec_verify -- --corpus target/ternary-corpus
```

- `external.truth_corpus`：171 行真值表逐行用评估工具内的独立代算复核（3 种代数 × 57 组合）；
- `external.truth_complete`：键集合去重 + 总数 171，缺行、重复行都红；
- `external.balanced_corpus`：85 行同时过独立文本解析、库解码、库重新编码三条；
- 带 `--corpus` 时总计 **24 项检查 / 58,945 例 / 0 失败**；不带时 21 项 / 58,517 例。

### 3.3 WSP 模型级 GGUF 复验（新增）

`gguf_verify` 只审 `trit-export` 的合成张量（导出层）；模型级 67/103 的旧账由
`trit-wspace gguf-audit` 复验（同一 all-MiniLM-L6-v2，103 个 2D 张量）：

```bash
trit-wspace gguf-audit <wsp1-dir> --out target/gguf-audit.json
```

| 读数 | 值 |
|---|---:|
| 张量数（导入 / 往返后） | 103 / 103 |
| 逐字节一致 `.wsp` | 67 / 103 |
| 语义核心一致（trits / 三值 bias / scale 位模式） | 103 / 103 |
| `biasf` 丢失 | 36 / 103 |
| manifest `vector` 标记保留 | 0 / 63 |
| manifest `bias` 标记保留 | 0 / 37 |
| GGUF 字节 / 有效张量字节 | 4,772,288 / 4,762,066 |

`ok=true` 只到语义核心；逐字节 67/103 与 36 个 `biasf` 丢失是预期损失，审计列入
`biasf_mismatches` 而不是判红。manifest 的 `vector` / `bias` 标记不写回，属于 import 侧
元数据缺口，不能把「导出层 4 项全绿」当成模型层全绿。

## 4. 归类与二进制基线（按 TERNARY-BOUNDARIES 规则先归类）

- **往返 / 纠错 / 检错 / 边界拒绝 / 语料完整性**：`[D]` 基数无关的复杂度——二进制同样要付，
  这些读数不能记进「三进制红利」。GGUF 模型级的 67/103 逐字节一致与 `biasf` 丢失同样归 `[D]`：
  这是容器表达力/宿主 f32 附件的取舍，与进制基数无关；语义核心 103/103 才是行为口径。
- **5 trits/byte 的 1.6 bit/trit**：`[B]` 宿主二进制上的编码税，不是三进制本身的缺陷。
  二进制基线：1 bit/值；三元 1.6 bit/trit，对比 `log2(3) ≈ 1.58496 bit/trit` 的信息上界，
  达成率约 **99.06%**。省的是**存储与带宽**，不省指令数。
- **`quantize` 的 3 电平重建**：`[B]`。双导轨三元的实际预算是 2 bit/trit，同预算二进制
  四电平基线的读数已在上表并列；打包后 1.6 bit/trit 的每比特折算也在表里。量化误差本身
  是设计目标（有损压缩），不是 [A]；本次也没有「三值权重比二值/四值训练更好」的证据。
- **没有 `[A]` 声明**：本次没有任何「原生三值器件」证据，也没有把 B 类开销冒充成 A 类红利。
- **没有新增 `[C]`**：评估只读既有 API，不引入第二套状态表示。

## 5. 复算

```bash
cargo run -q -p trit-codec --bin codec_verify -- --json          # 全量（21 项 / 58,517 例）
cargo run -q -p trit-codec --bin codec_verify -- --quick        # 快速档
cargo run -q -p trit-codec --bin codec_verify -- --json --corpus target/ternary-corpus
cargo run -q -p trit-export --bin gguf_verify -- --json
```

退出码：0 全绿，2 有失败（首个反例进报告），64 参数/语料错误。

## 未证栏

- **性能**：没有测吞吐/延迟，故意不给数——没有同机二进制基线就不是结论。
- **量化语料是合成的**：6 种分布 × 4 长度 × 4 beta，不是真实模型权重；真实权重的
  分布偏移、逐层 scale、训练后精度收益都未测。beta 只测了 4 个点，未做连续扫描。
- **2 符号错误**：Hamming `[4,2,3]` 只保证纠 1 个符号；双错可能被误纠而不是报错，
  这是码距的定义决定的，本次不测也不宣称能检出。
- **CRC-3 多符号/突发错误**：只穷举了单符号错误；双符号与突发错误的检出率未测。
- **超长样本**：`checksum.detects_single_symbol` 对长度 >300 的样本是抽样 16 个位置，
  不是全穷举；更大规模（>1024 trit）没有专门语料。
- **跨语言差分只走到「有消费者」**：`--corpus` 会复核 CSV 的语义与格式，但第三方语言
  实现尚未实际接入跑差分；这仍是外部动作，不能记成对拍读数。
- **容器上限**：合成张量只审到 33×129（4,257 trit）；模型级已审 all-MiniLM-L6-v2 的 103 个
  张量（最大 30522×384），但更大模型、32 字节对齐边界与超大张量仍未覆盖。
