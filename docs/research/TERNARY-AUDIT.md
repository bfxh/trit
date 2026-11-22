# 三进制专项评估：编码可逆与逐位一致（2026-11-23）

> 定位：对 `trit-codec` 的 10 类编码器与 `trit-export` 的 GGUF v3 容器做一次**独立复核**——
> 不是「编码器再解一遍能对上」，而是另写最小实现与库输出逐位比对。谁该读：改 codec / 容器的
> 人、准备引用「编码可逆」这句话的人。
>
> 读它解决的问题：仓里多处写着「大多数编码器是无损的」「GGUF 往返与 trit-codec 逐位一致」，
> 这些句子此前只有零散单测支撑，没有一份可一次跑完、失败即报红的对账。

## 边界

- **只审可逆与位级一致**：往返、独立 oracle、纠错/检错的最基本不变式、非法输入拒绝。
- **不审性能**：不给吞吐、不给压缩率优劣结论；要谈快慢必须另行同机二进制基线。
- **不审有损路径**：`quantize` 的设计目的就是有损，不在「可逆」口径内，本次未覆盖。
- **不审硬件 / 器件**：本仓不做原生三值器件仿真，本评估也不包含任何「原生红利」声明。
- **不替代**：单测、`tests/stress.rs`、`trit-fuzz` 各自覆盖的面；本评估只做它们没做完的交叉对账。

## 1. 口径：什么叫「独立复核」

三条判据，缺一条就不算复核：

1. **往返**：`encode → decode` 逐位还原（含空输入与长度边界）。
2. **独立 oracle**：在评估工具里另写一份最小实现，与库输出**逐位比对**——
   位级打包（`byte = Σ trit[i]·3^i`）、平衡三进制文本解析、mod 3 差分还原、稀疏 COO 重建。
   这条防的是「编码器与解码器同错，往返照样自洽」。
3. **不变式与拒绝**：Hamming 单符号错必须纠正并报出位置；CRC-3 单符号错必须检出；
   非法字节 / 越界索引 / 坏块长 / 零游程必须显式报错，不许猜。

独立 oracle 与库共享的是**线格式契约**（`trit-core` 的 pack 模块文档），不是实现代码。

## 2. 语料与种子

- 形态 9 种：全 False / 全 True / 全 Unknown / 0-1 交替 / 0-X 交替 / 1-X 交替 / 稀疏（3% 有定义）/
  成段（每 5 个一段）/ 均匀随机。
- 长度 25 档：0、1、2、3、4、5、6、7、9、10、11、12、24、25、26、49、50、51、127、128、129、
  255、256、257、1024（覆盖 5 trits/byte 的字节边界）。
- 合计 225 个样本 / 22,239 个 trit；种子 `0x9e3779b97f4a7c15`，混入形态号与长度，**同种子逐位可复现**。
- GGUF 侧：1..6 行 × 1..20 列的全形状矩阵、全 Unknown / 全 True / 全 False 病态张量、
  33×129 大张量，以及 scale 的位模式敏感值（`-0.0`、最小正规、最小次正规、NaN）。

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

细节口径：

- `checksum.detects_single_symbol`：长度 ≤300 的样本穷举「每个位置 × 两种替换值」；
  更长的样本每个抽 16 个位置——**不是**全穷举，见未证栏。
- `gguf.metadata_and_determinism`：三次检查——同模型两次写出字节相同、读回后逐字段相同、
  读回再写出的字节与首次相同（NaN 与 `-0.0` 按 `to_bits` 比较）。
- `gguf.pack_bit_parity`：导出前后的 trit 用 `trit_core::pack` 打字节，必须完全相同；
  且这份字节要与评估工具里的独立打包器一致。

## 4. 归类与二进制基线（按 TERNARY-BOUNDARIES 规则先归类）

- **往返 / 纠错 / 检错 / 边界拒绝**：`[D]` 基数无关的复杂度——二进制同样要付，
  这些读数不能记进「三进制红利」。
- **5 trits/byte 的 1.6 bit/trit**：`[B]` 宿主二进制上的编码税，不是三进制本身的缺陷。
  二进制基线：1 bit/值（每值 1 bit，信息率 100%）；三元 1.6 bit/trit，对比
  `log2(3) ≈ 1.58496 bit/trit` 的信息上界，达成率约 **99.06%**。省的是**存储与带宽**，
  不省指令数（编码要做乘 3 与取模）。
- **没有 `[A]` 声明**：本次没有任何「原生三值器件」证据，也没有把 B 类开销冒充成 A 类红利。
- **没有新增 `[C]`**：评估只读既有 API，不引入第二套状态表示。

## 5. 复算

```bash
cargo run -q -p trit-codec --bin codec_verify -- --json          # 全量
cargo run -q -p trit-codec --bin codec_verify -- --quick        # 快速档（72 样本）
cargo run -q -p trit-export --bin gguf_verify -- --json
```

退出码：0 全绿，2 有失败（首个反例进报告），64 参数错误。

## 未证栏

- **性能**：没有测吞吐/延迟，故意不给数——没有同机二进制基线就不是结论。
- **2 符号错误**：Hamming `[4,2,3]` 只保证纠 1 个符号；双错可能被误纠而不是报错，
  这是码距的定义决定的，本次不测也不宣称能检出。
- **CRC-3 多符号/突发错误**：只穷举了单符号错误；双符号与突发错误的检出率未测。
- **超长样本**：`checksum.detects_single_symbol` 对长度 >300 的样本是抽样 16 个位置，
  不是全穷举；更大规模（>1024 trit）没有专门语料。
- **跨语言差分**：`trit-codec/examples/ternary_corpus.rs` 产出的黄金语料是另一条线
  （给独立实现做差分），本次没有把它的 CSV 回灌进评估。
- **有损路径**：`quantize` 的误差与反量化一致性未审。
- **容器上限**：GGUF 最大只审到 33×129（4,257 trit）；32 字节对齐的边界、超大张量、
  以及 `trit-wspace` 的模型级落点不在本次范围。
