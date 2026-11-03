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


def family_line(fam, stats=None):
    """族在总表里的一行（live 报本族扫描数；registered 报缺什么）。"""
    st = fam.get("status", "registered")
    if st == "live":
        line = (
            f"- **{fam['title']}**（live）：清单 + 独立复核已接线；"
            f"完整表见 `{fam.get('detail', '')}`"
        )
        if stats:
            runs, ok = stats
            line += f"；本次扫描 {runs} 条运行，{ok} 条通过独立复核"
        return line
    return f"- **{fam['title']}**（registered，产物未接）：{fam.get('needs', '')}"


def verify_sibling(m):
    """清单里登记的产物旁边的复核记录（PASS 才会存在）。"""
    art = m.get("deploy") or m.get("model") or {}
    p = art.get("path")
    return pathlib.Path(str(p) + ".verify.json") if p else None


def family_stats(fam):
    """按族扫描：清单数 / 其中产物旁有 `.verify.json` 的数。"""
    root = pathlib.Path(fam.get("dir", ""))
    kind = fam.get("manifest_kind")
    runs, ok = 0, 0
    if fam.get("dir") and root.exists():
        for mf in root.rglob("*.manifest.json"):
            m = load(mf)
            if not m or (kind and m.get("kind") != kind):
                continue
            runs += 1
            v = verify_sibling(m)
            if v and v.exists():
                ok += 1
    return runs, ok


def fmt_parts_cfg(cfg):
    parts = [f"{cfg.get('res', '?')}²", f"k{cfg.get('k', '?')}"]
    if cfg.get("seg_sub") and cfg["seg_sub"] != "seg":
        parts.append(str(cfg["seg_sub"]))
    if cfg.get("bnd_weight"):
        parts.append(f"bnd×{cfg['bnd_weight']:g}")
    if cfg.get("accum", 1) > 1:
        parts.append(f"accum {cfg['accum']}")
    parts.append(f"lr {cfg.get('lr', '?')}")
    parts.append(f"{cfg.get('steps', '?')} 步")
    return " · ".join(str(p) for p in parts)


def parts_rows(root):
    """(清单, 复核记录或 None, 复核是否指向同一模型)。"""
    rows = []
    for mf in sorted(root.rglob("*.manifest.json")) if root.exists() else []:
        m = load(mf)
        if not m or m.get("kind") != "parts-train":
            continue
        deploy = m.get("deploy") or {}
        sha = deploy.get("sha256")
        vp = verify_sibling(m)
        v = load(vp) if vp and vp.exists() else None
        # 「通过复核」= 模型相同 **且** 复核记录确实锚在一份清单上（A1 是族进 live 的硬条件）
        same = (
            bool(v)
            and bool(sha)
            and (v.get("model") or {}).get("sha256") == sha
            and bool((v.get("manifest") or {}).get("sha256"))
        )
        rows.append((m, v, same))
    rows.sort(key=lambda r: r[0].get("created_unix", 0), reverse=True)
    return rows


def parts_doc(root):
    rows = parts_rows(root)
    out = []
    out.append("# 部件头结果目录（自动生成）\n")
    out.append(
        "> 由 `scripts/depth_catalogue.py` 从 `runs/parts/*.manifest.json` 生成，**不要手改**。"
        "每行的「复核」来自独立进程 `parts-verify`：它重读训练集与评测语料、重算内容级 "
        "SHA-256、重跑 `parts-inst eval`；指纹或模型与清单不一致会判红（写 `.verify.reject.json`）。\n"
    )
    out.append("## 结果族：部件平面头 / 分割与实例\n")
    if not rows:
        out.append("（还没有带清单的运行）\n")
        return out
    out.append(
        "| 模型 | 配置 | 训练数据（场景/帧） | 训练 loss | 留出复核（帧 / mIoU / 前景 IoU / 边界 F1） | 指纹（模型前 12） | 来源 | 时间 |"
    )
    out.append("|---|---|---|---|---|---|---|---|")
    for m, v, same in rows:
        cfg = fmt_parts_cfg(m.get("config", {}))
        d = m.get("data", {})
        loss = m.get("result", {}).get("final_loss")
        loss_s = f"{loss:.4f}" if isinstance(loss, (int, float)) else "?"
        if v and same:
            met = v.get("metrics", {})
            check = (
                f"✅ {met.get('frames', '?')} 帧 / {met.get('miou', 0):.4f} / "
                f"{met.get('fg_iou', 0):.3f} / {met.get('bnd_f1', 0):.3f}"
            )
        elif v:
            check = "⚠️ 复核记录与模型不符"
        else:
            check = "未复核"
        deploy = m.get("deploy") or {}
        sha = str(deploy.get("sha256", ""))[:12]
        commit = str(m.get("commit", "?")) + ("+dirty" if m.get("dirty") else "")
        ts = m.get("created_unix", 0)
        time_s = (
            __import__("datetime")
            .datetime.fromtimestamp(ts, __import__("datetime").UTC)
            .strftime("%Y-%m-%d %H:%M UTC")
            if ts
            else "?"
        )
        p = pathlib.Path(str(deploy.get("path", "?")))
        # 部件产物都叫 model.tpm，用「目录/文件」区分不同运行
        name = f"{p.parent.name}/{p.name}" if p.parent.name else p.name
        out.append(
            f"| `{name}` | {cfg} | {d.get('dirs', '?')} / {d.get('frames', '?')} | {loss_s} | "
            f"{check} | `{sha}` | `{commit}` | {time_s} |"
        )
    out.append("")
    out.append("## 如何复核一行\n")
    out.append("```sh")
    out.append(
        "parts-verify --model <模型.tpm> --data <训练时的场景根目录列表> \\\n"
        "  --seg-sub <seg/pbnd> --renders <留出 renders> --scenes <留出 scenes> \\\n"
        '  --eval-args "--gt part --boundary --k 5 --res 384"'
    )
    out.append("```\n")
    out.append(
        "复核器会重算 `sha256(模型)`、训练集与评测语料的内容级指纹，与 "
        "`<模型>.manifest.json` 比对；一致才写 `.verify.json`，否则写 "
        "`.verify.reject.json` 并退出码 1。\n"
    )
    return out


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
    live = family_line({"title": "深度", "status": "live", "detail": "docs/D.md"})
    assert "live" in live and "docs/D.md" in live and "本次扫描" not in live
    live2 = family_line({"title": "深度", "status": "live", "detail": "docs/D.md"}, (9, 9))
    assert "9 条运行" in live2 and "9 条通过" in live2, live2
    reg = family_line({"title": "部件", "status": "registered", "needs": "写清单"})
    assert "registered" in reg and "写清单" in reg and "完整表" not in reg
    # 部件配置行：只报非默认项 + 步数
    pc = fmt_parts_cfg({"res": 384, "k": 5, "seg_sub": "pbnd", "bnd_weight": 4, "accum": 4, "lr": 0.001, "steps": 600})
    assert "384²" in pc and "pbnd" in pc and "bnd×4" in pc and "accum 4" in pc and "600 步" in pc, pc
    pc2 = fmt_parts_cfg({"res": 384, "k": 5, "seg_sub": "seg", "bnd_weight": 0, "accum": 1, "lr": 0.001, "steps": 10})
    assert "seg" not in pc2 and "bnd×" not in pc2 and "accum" not in pc2, pc2
    assert verify_sibling({"deploy": {"path": "E:/runs/x.tpm"}}) == pathlib.Path("E:/runs/x.tpm.verify.json")
    assert verify_sibling({"model": {"path": "E:/runs/y.dcn"}}) == pathlib.Path("E:/runs/y.dcn.verify.json")
    both = {"model": {"path": "E:/runs/m.dcn"}, "deploy": {"path": "E:/runs/d.tpm"}}
    assert verify_sibling(both) == pathlib.Path("E:/runs/d.tpm.verify.json"), verify_sibling(both)
    assert verify_sibling({}) is None
    # 复核记录必须同时满足：模型 SHA 相同 + 锚在清单上
    import tempfile

    with tempfile.TemporaryDirectory() as td:
        root = pathlib.Path(td)
        sha = "ab" * 32
        man = {"kind": "parts-train", "created_unix": 1, "deploy": {"path": str(root / "x.tpm"), "sha256": sha}}
        (root / "x.tpm.manifest.json").write_text(json.dumps(man), encoding="utf-8")
        assert parts_rows(root)[0][2] is False  # 没有复核记录
        (root / "x.tpm.verify.json").write_text(
            json.dumps({"model": {"sha256": sha}, "manifest": {}}), encoding="utf-8"
        )
        assert parts_rows(root)[0][2] is False  # 没锚在清单上
        (root / "x.tpm.verify.json").write_text(
            json.dumps({"model": {"sha256": "cd" * 32}, "manifest": {"sha256": "e" * 64}}),
            encoding="utf-8",
        )
        assert parts_rows(root)[0][2] is False  # 模型不符
        (root / "x.tpm.verify.json").write_text(
            json.dumps({"model": {"sha256": sha}, "manifest": {"sha256": "e" * 64}}),
            encoding="utf-8",
        )
        assert parts_rows(root)[0][2] is True
    print("self-test 11/11 通过")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--dir", default="runs/depth")
    ap.add_argument("--out", default="docs/DEPTH-RESULTS.md")
    ap.add_argument("--families", default="sweeps/families.json")
    ap.add_argument("--umbrella", default="docs/RESULTS.md", help="总表输出（空串 = 不写）")
    ap.add_argument("--parts-dir", default="runs/parts")
    ap.add_argument("--parts-out", default="docs/PARTS-RESULTS.md", help="部件族表（空串 = 不写）")
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
    if args.parts_out:
        proot = pathlib.Path(args.parts_dir)
        pd = parts_doc(proot)
        pathlib.Path(args.parts_out).write_text("\n".join(pd) + "\n", encoding="utf-8")
        print(f"已写 {args.parts_out}（{len(parts_rows(proot))} 行）")

    if args.umbrella:
        fam_path = pathlib.Path(args.families)
        fams = []
        if fam_path.exists():
            fams = load(fam_path).get("families", [])
        u = []
        u.append("# 结果目录（自动生成）\n")
        u.append(
            "> 由 `scripts/depth_catalogue.py` 生成，**不要手改**。只有 `live` 的族有"
            "机器可读清单 + 独立复核；`registered` 的族只登记协议与证据位置，"
            "文档读数**不算复核**。\n"
        )
        u.append("## 已接线（live）\n")
        live_rows = [f for f in fams if f.get("status") == "live"]
        if not live_rows:
            u.append("（无）\n")
        for f in live_rows:
            u.append(family_line(f, family_stats(f)))
        u.append("")
        u.append("## 已登记、产物未接（registered）\n")
        reg_rows = [f for f in fams if f.get("status") != "live"]
        if not reg_rows:
            u.append("（无）\n")
        for f in reg_rows:
            u.append(family_line(f))
            ev = f.get("evidence")
            if ev:
                u.append(f"  - 文档证据：{ev}")
        u.append("")
        pathlib.Path(args.umbrella).write_text("\n".join(u) + "\n", encoding="utf-8")
        print(f"已写 {args.umbrella}（{len(live_rows)} live / {len(reg_rows)} registered）")


if __name__ == "__main__":
    main()
