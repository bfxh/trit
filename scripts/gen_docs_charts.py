#!/usr/bin/env python3
"""生成 README 顶部的对比图（SVG，无外部依赖）。

数据全部来自仓库内实测，改数字时**同时**改这里的注释来源和对应 crate README，
不要从别的机器上抄读数。

    python scripts/gen_docs_charts.py
"""

from __future__ import annotations

from pathlib import Path

OUT = Path(__file__).resolve().parents[1] / "docs" / "assets"

GRAY = "#8a8f98"
GREEN = "#1f7a4d"
RED = "#b3541e"


def bar_chart(
    name: str,
    title: str,
    subtitle: str,
    rows: list[tuple[str, float, str]],
    fmt: str,
    note: str,
) -> None:
    width, left, right, top = 940, 260, 150, 92
    bar_h, gap = 30, 16
    height = top + len(rows) * (bar_h + gap) + 56
    vmax = max(value for _, value, _ in rows)
    span = width - left - right
    parts = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" '
        f'viewBox="0 0 {width} {height}" font-family="-apple-system,Segoe UI,Helvetica,Arial,sans-serif">',
        f'<rect width="{width}" height="{height}" fill="#ffffff"/>',
        f'<text x="{left}" y="38" font-size="22" font-weight="700" fill="#111">{title}</text>',
        f'<text x="{left}" y="64" font-size="14" fill="#666">{subtitle}</text>',
    ]
    for index, (label, value, color) in enumerate(rows):
        y = top + index * (bar_h + gap)
        w = max(2, round(span * value / vmax))
        parts.append(
            f'<text x="{left - 14}" y="{y + bar_h * 0.7:.0f}" text-anchor="end" '
            f'font-size="14" fill="#222">{label}</text>'
        )
        parts.append(f'<rect x="{left}" y="{y}" width="{w}" height="{bar_h}" rx="4" fill="{color}"/>')
        parts.append(
            f'<text x="{left + w + 10}" y="{y + bar_h * 0.7:.0f}" font-size="14" '
            f'font-weight="600" fill="#111">{fmt.format(value)}</text>'
        )
    parts.append(f'<text x="{left}" y="{height - 18}" font-size="12" fill="#777">{note}</text>')
    parts.append("</svg>")
    (OUT / name).write_text("\n".join(parts) + "\n", encoding="utf-8")


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)

    # crates/trit-ops/README.md「实测一：流式」：GMAC/s ÷ f32 = 倍数。
    bar_chart(
        "speedup.svg",
        "CPU 流式吞吐：相对 f32 的倍数",
        "1.34 亿参数一次过一遍（工作集远大于缓存）；越高越好",
        [
            ("f32（基线）", 1.00, GRAY),
            ("f16", 2.09, GRAY),
            ("Q4_0", 7.45, GRAY),
            ("三值 · 双导轨掩码", 10.49, GREEN),
            ("三值 · tryte 查表", 15.17, GREEN),
            ("三值 · 自动选核", 15.83, GREEN),
        ],
        "{:.2f}×",
        "同机 24 核 / 8 线程；来源 crates/trit-ops/README.md 实测一（2026-10-25）",
    )

    # 同表：每参数字节。
    bar_chart(
        "bits.svg",
        "每参数存储：字节数",
        "越低越好；1.34 亿参数从 537 MB（f32）到 27 MB（tryte）",
        [
            ("f32", 4.0, GRAY),
            ("f16", 2.0, GRAY),
            ("Q4_0", 0.5, GRAY),
            ("三值 · 双导轨", 0.25, GREEN),
            ("三值 · tryte", 0.2, GREEN),
        ],
        "{:.2f} B",
        "来源同左表；三值权重无浮点乘法，掩码内核按位加减",
    )

    # crates/trit-gpu/README.md：512×4096×512，ps/MAC，越低越好。
    bar_chart(
        "gpu.svg",
        "GPU 矩阵乘：ps/MAC",
        "RTX 4060 Ti，512×4096×512；越低越好",
        [
            ("f32 最优（4×4 + K32）", 0.298, GRAY),
            ("三值 · 3-popc", 0.104, GREEN),
            ("三值 · split-K×4", 0.101, GREEN),
        ],
        "{:.3f} ps",
        "来源 crates/trit-gpu/README.md（2026-10-25）；三值比 f32 最优快 2.9×",
    )



    # trit-float range（32 比特口径，2026-10-26）：log10 动态范围与精度（十进制位）。
    bar_chart(
        "float-range.svg",
        "32 比特下的动态范围（log10）",
        "越高越好；三值的指数范围增长快，但会挤占尾数",
        [
            ("IEEE single", 83.4, GRAY),
            ("三值 · 匹配尾数位", 45.0, GREEN),
            ("三值 · 匹配指数位", 353.2, GREEN),
        ],
        "{:.1f}",
        "来源 crates/trit-float（trit-float range）；范围为 max/min 的十进制位数",
    )

    bar_chart(
        "float-precision.svg",
        "32 比特下的精度（十进制有效位）",
        "越高越好；动态范围是用这里换来的",
        [
            ("IEEE single", 6.92, GRAY),
            ("三值 · 匹配尾数位", 6.68, GREEN),
            ("三值 · 匹配指数位", 5.73, RED),
        ],
        "{:.2f}",
        "来源同上；三值误差上界 3^{1-p}，折算成十进制位数 (p-1)·log10 3",
    )


if __name__ == "__main__":
    main()
