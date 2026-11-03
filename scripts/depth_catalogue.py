#!/usr/bin/env python3
"""把 `runs/depth/` 里的清单与复核记录生成结果目录（A3）。

用法:
    python scripts/depth_catalogue.py --dir runs/depth --out docs/DEPTH-RESULTS.md

目录只收录带**机器可读清单**的运行；「复核」列来自 `parts-depth-verify` 写的
`<model>.verify.json`（PASS 才会落这个文件）。没有复核记录的记作「未复核」。
"""

import argparse
import json
import pathlib


def load(path):
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except Exception as e:  # noqa: BLE001 - 目录工具，坏文件要指名而不是崩
        print(f"跳过坏文件 {path}: {e}")
        return None


def promotion_mark(model_path, promoted):
    """模型是否在 promoted 集合里（目录的「晋级」列）。"""
    return "✓" if pathlib.Path(model_path).resolve() in promoted else "—"


def fmt_cfg(cfg):
    parts = [f"h{cfg.get('hidden', '?')}"]
    for k, label in (("dil", "dil"), ("pyr", "pyr"), ("accum", "accum"), ("lr", "lr")):
        v = cfg.get(k)
        if v not in (None, 0, 1) or k == "lr":
            parts.append(f"{label} {v}")
    if cfg.get("f32"):
        parts.append("f32")
    if cfg.get("full"):
        parts.append("full")
    parts.append(f"{cfg.get('epochs', '?')} ep")
    return " · ".join(str(p) for p in parts)


def self_test():
    cfg = {"hidden": 32, "dil": 2, "pyr": 0, "accum": 1, "lr": 0.002, "epochs": 3}
    a = fmt_cfg(cfg)
    assert "h32" in a and "lr 0.002" in a and "pyr" not in a and "accum" not in a, a
    cfg2 = {"hidden": 64, "dil": 4, "pyr": 2, "accum": 8, "lr": 0.001, "epochs": 10, "f32": True, "full": True}
    b = fmt_cfg(cfg2)
    assert "pyr 2" in b and "accum 8" in b and "f32" in b and "full" in b, b
    p = {pathlib.Path("E:/runs/x.dcn").resolve()}
    assert promotion_mark("E:/runs/x.dcn", p) == "✓"
    assert promotion_mark("E:/runs/y.dcn", p) == "—"
    print("self-test 4/4 通过")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--dir", default="runs/depth")
    ap.add_argument("--out", default="docs/DEPTH-RESULTS.md")
    ap.add_argument("--self-test", action="store_true")
    args = ap.parse_args()
    if args.self_test:
        self_test()
        return
    root = pathlib.Path(args.dir)
    rows = []
    for mf in sorted(root.rglob("*.manifest.json")):
        m = load(mf)
        if not m:
            continue
        model = pathlib.Path(m["model"]["path"])
        verify = model.with_name(model.name + ".verify.json")
        reject = model.with_name(model.name + ".verify.reject.json")
        v = load(verify) if verify.exists() else None
        rj = load(reject) if reject.exists() else None
        rows.append((m, v, rj))
    rows.sort(key=lambda r: r[0].get("created_unix", 0), reverse=True)

    promoted = {}
    for pf in root.rglob("promoted.json"):
        d = load(pf)
        if not d:
            continue
        for tag in d.get("promoted", []):
            promoted[(pf.parent / f"{tag}.dcn").resolve()] = d.get("rule")

    out = []
    out.append("# 深度头结果目录（自动生成）\n")
    out.append(
        "> 由 `scripts/depth_catalogue.py` 从 `runs/depth/*.manifest.json` 生成，"
        "**不要手改**。每行的「复核」来自独立进程 `parts-depth-verify`：它重读数据、"
        "重算模型/数据指纹（数据是**内容级** SHA-256）并在留出集上重跑评测；指纹与清单不一致会判红。\n"
    )
    out.append("## 结果族：单目深度头\n")
    if not rows:
        out.append("（还没有带清单的运行）\n")
    else:
        out.append("| 模型 | 配置 | 数据（目录/文件） | 训练 MSE | 留出复核（形状对齐 / δ<1.25 / 常数基线） | 晋级 | 指纹（模型前 12） | 来源 | 时间 |")
        out.append("|---|---|---|---|---|---|---|---|---|")
        for m, v, rj in rows:
            d = m.get("data", {})
            cfg = fmt_cfg(m.get("config", {}))
            mse = m.get("result", {}).get("final_mse")
            mse_s = f"{mse:.4f}" if isinstance(mse, (int, float)) else "?"
            if v:
                met = v.get("metrics", {})
                check = (
                    f"✅ {met.get('rel_pct', 0):.2f}% / {met.get('delta', 0):.3f} / "
                    f"{met.get('base_pct', 0):.2f}%"
                )
            elif rj:
                met = rj.get("metrics", {})
                check = f"❌ 指纹不一致（{met.get('rel_pct', 0):.2f}%）"
            else:
                check = "未复核"
            sha = m.get("model", {}).get("sha256", "")[:12]
            commit = m.get("commit", "?") + ("+dirty" if m.get("dirty") else "")
            ts = m.get("created_unix", 0)
            time_s = (
                __import__("datetime").datetime.fromtimestamp(ts, __import__("datetime").UTC).strftime("%Y-%m-%d %H:%M UTC")
                if ts
                else "?"
            )
            name = pathlib.Path(m.get("model", {}).get("path", "?")).name
            mp = pathlib.Path(m.get("model", {}).get("path", "")).resolve()
            promo = promotion_mark(m.get("model", {}).get("path", ""), promoted)
            out.append(
                f"| `{name}` | {cfg} | {d.get('dirs', '?')} / {d.get('files', '?')} | {mse_s} | "
                f"{check} | {promo} | `{sha}` | `{commit}` | {time_s} |"
            )
    out.append("")
    out.append("## 如何复核一行\n")
    out.append("```sh")
    out.append("parts-depth-verify --dcn <模型> --data <训练时的 --data 列表> \\")
    out.append("  --holdout <清单里的 holdout> --size <config.size>")
    out.append("```\n")
    out.append(
        "复核器会重算 `sha256(model)` 与数据集指纹，与 `<模型>.manifest.json` 比对；"
        "一致才写 `.verify.json`，不一致写 `.verify.reject.json` 并退出码 1。\n"
    )
    pathlib.Path(args.out).write_text("\n".join(out) + "\n", encoding="utf-8")
    print(f"已写 {args.out}（{len(rows)} 行）")


if __name__ == "__main__":
    main()
