# WSP3（RLE 二级压缩）实测：真实 MiniLM 上无收益

> 结论：WSP3 对“全同/低熵”工件有效（`tests/wsp3.rs` 的 8×32 全 `+1` 网络确实变小），
> 但对真实三值权重**反而比 WSP2 大 0.7%**；2-bit 已经接近熵下限。RLE 不是这块的正解，
> 暂不继续做熵编码。

## 1. 读数（all-MiniLM-L6-v2，103 个张量）

| 容器 | 总字节 | 相对 WSP1 | 相对 WSP2 |
|---|---:|---:|---:|
| WSP1（文本） | 46,608,788 | 100% | — |
| WSP2（2-bit） | 5,985,699 | 12.8% | 100% |
| WSP3（WSP2 + PackBits RLE） | 6,025,083 | 12.9% | **100.7%** |

复算：

```bash
trit-wspace import-safetensors-dir model.safetensors --out model/
for f in model/*.wsp; do
  trit-wspace pack-artifact "$f" --out "${f%.wsp}.wsp2"
  trit-wspace pack-artifact "$f" --out "${f%.wsp}.wsp3" --rle
done
stat -c %s model/*.wsp2 | awk '{s+=$1} END {print s}'
```

本机读数：`WSP1=46608788 WSP2=5985699 (12%) WSP3=6025083 (100% of WSP2)`。

## 2. 为什么 RLE 无效

同一批 WSP1 的 trit 序列（22,713,216 个）：

| 项 | 读数 |
|---|---:|
| 分布 `0 / + / -` | 31.4% / 34.3% / 34.3% |
| 平均游程 | 1.500 |
| 游程 ≥ 3 的比例 | 11.1% |
| 熵 | 1.584 bit/trit |
| 固定 2-bit | 2 bit/trit |
| 熵编码理论下限 | 79.2% of WSP2 |

平均游程 1.5、游程 ≥3 只占 11% ⇒ 字节级与三值级 RLE 都拿不到收益；
2-bit 与熵下限只差 20.8%，继续压需要 Huffman/算术编码（成本高、收益有限）。

## 3. 边界

- WSP3 保留给低熵/全同工件；真实权重不要用 `--rle`，WSP2 已到 12.8% of WSP1。
- 未证：其他模型（不同稀疏度）的熵可能更低；没有在更大模型/GPU 上复测。

## 4. WSP4：静态 Huffman（已实现，82.7% of WSP2）

`WSP4` = `magic "WSP4" | version 1 | method u8 | raw_len u64 | 256 码长 + 位流 | checksum`；
method=0 原样存（压缩不更小时回退）。

| 容器 | 总字节 | 相对 WSP2 |
|---|---:|---:|
| WSP2 | 5,985,699 | 100% |
| WSP3（RLE） | 6,025,083 | 100.7% |
| **WSP4（Huffman）** | **4,953,834** | **82.7%** |

同一批 103 个 WSP1，`pack-artifact --huffman` 生成 `.wsp4` 后求和；82.7% 与字节熵下限 82.0%
相差 0.7 pp（码表头 + 位流填充）。

无损验证：103 个 `.wsp4` 全部 `unpack-artifact` 回 WSP1 后，与原始 `.wsp` **逐字节一致**
（`cmp -s` 103/103 通过）。

结论：熵编码是这块的正解；RLE 只适合低熵/全同工件。

## 5. rANS 原型：不优于 Huffman（负结果）

静态 rANS（字节级，M=2^12，32 位状态）原型对同一批 103 个 WSP2：

| 编码 | 总字节 | 相对 WSP2 |
|---|---:|---:|
| Huffman（WSP4） | 4,953,834 | 82.76% |
| rANS（原型） | 4,991,451 | 83.39% |

- 大文件（word_embeddings 3,059,855 字节）：Huffman 2,509,647 vs rANS 2,519,115（rANS +0.38%）。
- 160KB 文件（intermediate.dense）：rANS 133,835 vs Huffman 134,520（rANS −0.5%）。
- 原因：归一化到 4096 的量化误差 + 512B 频率表；Huffman 的整数码长损失在真实数据上反而更小。
- 结论：不实现 rANS（收益为负）；WSP4 保持 Huffman + method=0 回退。
