# 整模型批量容器 WSPM

状态：**已实现**（`wspm` 模块 + CLI `pack-model` / `unpack-model`）。
把一个 WSP1 模型目录（`manifest.json` + `NNNN-<name>.wsp`，与
`import-safetensors-dir` / `import-gguf` 的目录约定一致）打包成单个二进制文件，
`unpack` 逐字节还原。此前整模型压缩只有单工件 `pack-artifact`，103 个张量要手工循环。

## 1. 容器布局（小端）

```text
magic "WSPM" | version u8=1 | flags u8=0 | count u32
每 entry：name_len u16 | name | method u8 | vector u8 | bias_ref u32
          | rows u64 | cols u64 | offset u64 | len u64
blob 区：各 entry 的压缩工件
末尾：checksum u64（前面全部字节的 FNV-1a）
```

- `method`：0=WSP2、1=WSP3、2=WSP4。`--format auto`（默认）逐张量在 WSP2/WSP3/WSP4
  里选最小（平局归 WSP2）；`--format wsp2|wsp3|wsp4` 强制统一格式。
- `bias_ref` / `vector`：原样携带清单的 `bias` / `vector` 字段语义（引用解析成
  条目序号），`unpack` 重建的 manifest 与原目录条目一致。
- blob 是完整的 WSP2/3/4 工件，解码走 `decode_auto`（magic、版本、内部校验和全查），
  外层再有一道整文件 FNV。
- **`biasf`（f32 偏置幅度）保留**：blob 是 WSP2，而 WSP2 v2 存 `biasf`；因此 WSPM
  往返是文本级无损的。这一点与 GGUF 容器不同（`TYPE_TRIT` 只存三值 bias，见
  `docs/GGUF-TRIT.md` §4：103 个张量只有 67 个逐字节一致）。

## 2. CLI

```bash
trit-wspace pack-model <dir> --out <file.wspm> [--format auto|wsp2|wsp3|wsp4]
trit-wspace unpack-model <file.wspm> --out <dir>
```

`pack-model` 读 `<dir>/manifest.json`，逐条加载 `.wsp`（必须是单层 WSP1），编码后
经 `atomic_write` 落盘（临时文件 + 改名）；输出 JSON 报告容器字节数、源 WSP1 字节
数、张量数与格式分布。`unpack-model` 写出 `NNNN-<name>.wsp` 与重建的
`manifest.json`（`source` 指向容器，`skipped` 为空数组）。写路径走 `cli::run` 的
`--out` 写锁，防并发覆盖。

## 3. 真实读数（all-MiniLM-L6-v2，103 个张量）

| 项 | 值 |
|---|---:|
| WSP1 目录（`source_bytes`） | 46,608,788 B |
| 单文件 WSP2 总和 | 5,985,699 B |
| 单文件 WSP3 总和 | 6,025,083 B |
| 单文件 WSP4 总和 | 4,953,834 B |
| **WSPM 容器（auto）** | **4,959,752 B**（10.6% of WSP1，82.9% of WSP2 总和） |
| 往返后 `.wsp` 逐字节一致 | **103 / 103** |
| manifest 张量条目一致 | 103 / 103（含 `vector` / `bias`） |
| pack / unpack 耗时 | ~1.06 s / ~0.72 s（20 次内取 3 轮） |
| 格式分布（auto） | wsp2=50、wsp3=14、wsp4=39 |

容器比「单文件 WSP4 逐个存放」只多 5,918 B（0.12%），换来：单一文件、
逐张量格式自适应、清单语义（vector/bias）与 `biasf` 的无损往返、整文件校验和。
注意 WSP3 单文件总和多于 WSP2（RLE 对真实权重无收益，见 `docs/WSP3-RESULTS.md`），
auto 选择把它排除在大多数张量之外。

## 4. 边界（未证栏）

- `skipped`（导入报告）与原始 `source` 不进容器；重建 manifest 的 `source` 指向
  容器自身。WSPM 归档的是**权重空间**，不是导入报告。
- blob 全量读入内存，没有流式/mmap；容器不支持按张量随机读（与 `read-block` 的
  WSP2 索引正交，未串联）。
- `count` 上限 u32、张量名上限 u16 字节；名字唯一性在 pack/unpack 两端都校验。
- 与 GGUF 的互操作未证（同 `docs/GGUF-TRIT.md`）；MCP 工具未接。
- 锁是 advisory，约束范围同 `docs/WSP-WRITE-LOCK.md`。
