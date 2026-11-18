# 纹理结果目录（自动生成）

> 由 `scripts/depth_catalogue.py` 从 `runs/texture/*.manifest.json` 生成，**不要手改**。每行的「复核」来自独立进程 `ttx-verify`：它重算 PNG 语料的内容级 SHA-256、索引 SHA、逐图比 TTX 哈希/PSNR/最大误差、**重编码逐字节比对**，并重查采样一致性（压缩域采样器在纹素中心必须等于整图解码，repeat 跨边界回环）。协议：**固定参数、无逐图调参**；失败写 `pack.verify.reject.json`。

## 结果族：纹理（TTX2 资产包）

| 包 | 配置 | 语料（文件/字节） | 保真（PSNR / 最大误差） | 码率 bpp | 采样一致性 | 索引（前 12） | 来源 | 时间 |
|---|---|---|---|---|---|---|---|---|
| `tex1299/pack` | ttx2 · stride 5 | 1299 / 48972318 | ✅ 36.88 dB / 85 | 2.128 | ✅ 0/10537488 不符 | `b7f4d86af0cc` | `ff11bdc` | 2026-11-03 22:32 UTC |

## 如何复核一行

```sh
ttx-verify --pack <产包目录>   # 清单缺省 <包>/pack.manifest.json
```

