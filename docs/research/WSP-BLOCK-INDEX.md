# WSP2 块索引与随机访问

状态：**已实现**（`wsp2::BlockIndex` + CLI `index-artifact` / `read-block`）。
索引只记录层/行块偏移，不复制权重；WSP3/WSP4 是 WSP2 的外层压缩流，不能按块
随机读，索引器会显式拒绝。

## 1. 索引格式（JSON）

```json
{
  "magic": "WSP2-IDX",
  "version": 1,
  "source_bytes": 3059855,
  "source_checksum": 123,
  "block_rows": 64,
  "layers": [
    {
      "index": 0,
      "rows": 30522,
      "cols": 384,
      "has_bias_f32": false,
      "layer_offset": 7,
      "scales_offset": 16,
      "bias_f32_offset": null,
      "trits_offset": 122096,
      "bias_offset": 3053968,
      "end_offset": 3059848,
      "row_blocks": [
        {"row_start": 0, "row_end": 64, "trits_offset": 122096, "trits_len": 6144}
      ]
    }
  ]
}
```

- `layer_offset..end_offset` 是该层在 WSP2 中的字节范围；`scales/bias_f32/trits/bias`
  各有偏移。
- `row_blocks` 按行递增、无重叠、覆盖 `0..rows`；默认 64 行一块。
- `source_checksum` 是 WSP2 尾部 FNV-1a；`index-artifact` 扫描时做一次全量校验。

## 2. 读取语义

- `BlockIndex::read_layer` / `read_layer_io`：读整层。
- `BlockIndex::read_rows` / `read_rows_io`：读 `row_start..row_start+rows`；跨块时逐块拼接。
- 2-bit trits 的块起点向下取整到字节，解出前缀后截取目标行；bias 同理。
- `read_*_io` 只 `seek` + `read_exact` 目标块所需的 scales/trits/bias 字节。
- 默认轻量检查：文件长度 + 尾部校验和；`read-block --verify` 重跑 `scan` 做全量 FNV。

## 3. CLI

```bash
trit-wspace index-artifact model.wsp2 --out model.idx
trit-wspace read-block model.wsp2 --index model.idx --layer 0 \
  --row-start 0 --rows 64 --out part.wsp
```

`read-block` 输出单层 WSP1（`--out` 可继续编辑或走事务）；报告 JSON 含
`layer/row_start/rows/cols/layer_bytes`。

## 4. 真实读数（all-MiniLM-L6-v2，103 个张量）

| 项 | 值 |
|---|---:|
| WSP1 文本总字节 | 46,608,788 |
| WSP2 总字节 | 5,985,699 |
| 索引总字节（103 个 `.idx`） | 165,978（WSP2 的 2.77%） |
| word_embeddings 层 | 30522 × 384，477 块 |
| 整层字节 | 3,059,840 |
| 首块 64 行 trits + scales + bias | 6,144 + 256 + 16 = 6,416（整层的 0.21%） |

耗时（release，本机 20 次均值）：

| 命令 | 耗时 |
|---|---:|
| `read-block` 64 行 | 36 ms |
| `unpack-artifact` 整层 | 202 ms |

另一轮 10 次读数为 75 ms / 248 ms（本机波动），比值约 5.6–6.4×。`read-block` 的耗时
包含进程启动与输出 WSP1，不等于纯 I/O。

复算：

```bash
# 从 safetensors 导入 WSP1 目录后
for f in model/*.wsp; do
  trit-wspace pack-artifact "$f" --out "${f%.wsp}.wsp2"
  trit-wspace index-artifact "${f%.wsp}.wsp2" --out "${f%.wsp}.idx"
done
trit-wspace read-block model/0004-embeddings.word_embeddings.weight.wsp2 \
  --index model/0004-embeddings.word_embeddings.weight.idx \
  --layer 0 --row-start 0 --rows 64 --out part.wsp
```

## 5. 边界（未证栏）

- WSP3/WSP4 是 WSP2 外层压缩流（RLE / Huffman 整段），不能按块随机读；
  `index-artifact` 对它们报错。要随机读 WSP4 需要改成分块压缩（每块独立码表），
  会牺牲压缩率。
- 没有接 mmap：`read_*_io` 用 `seek + read_exact`；与 `trit-mmap` 结合仍未做。
- 块大小固定 64 行，没有按层/按字节的自适应块。
- 索引与源文件的一致性默认只查长度 + 尾部校验和；`--verify` 才做全量 FNV。
- 没有索引合并/目录级索引；103 个 `.idx` 是逐文件独立的。
- 只测 MiniLM 规模；更大模型/单层 >1GB 未复测。
