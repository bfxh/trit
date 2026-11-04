#!/usr/bin/env python3
"""N3 LOD 门：逐级 3D IoU + 导出体积比（读 `runs/extract/*/sweep.json`）。

用法:
    python scripts/lod_gate.py --sweep runs/extract/sweep-lod/sweep.json
    python scripts/lod_gate.py --sweep ... --min-iou 0.35 --ratio 4
    python scripts/lod_gate.py --self-test

判据（全过才绿，退出码 0）:
  1. 每一级都 `verified`（有独立复核记录），且清单里有三角面数；
  2. **最粗**一级的 3D IoU ≥ `--min-iou`（缺省 0.35）；
  3. 最粗一级的三角面总数 ≤ 最细一级 / `--ratio`（缺省 4 ⇒ ≤1/4）。
级别按 `config.cell` 排序：数值越大越粗。

⚠️ 口径：不同体素格的 IoU **不完全可比**（粗网格的边界体素占比小）。门槛只回答
「低精度档还有没有 0.35 的 3D 形状」，不回答「逐级等价」。
"""

import argparse
import json
import pathlib
import sys


def levels_from(sweep_path):
    """把 sweep.json + 各级清单读成按「粗 → 细」排序的级别表。"""
    d = json.loads(pathlib.Path(sweep_path).read_text(encoding="utf-8"))
    out = []
    for r in d.get("rows", []):
        cfg = r.get("config") or {}
        model = pathlib.Path(str(r.get("model", "")))
        mp = pathlib.Path(str(model) + ".manifest.json")
        lv = {
            "cell": cfg.get("cell"),
            "iou": (r.get("metrics") or {}).get("iou_mean"),
            "tris": None,
            "status": r.get("status"),
            "why": None,
        }
        if not mp.exists():
            lv["why"] = f"缺清单 {mp}"
        else:
            man = json.loads(mp.read_text(encoding="utf-8"))
            lv["tris"] = sum(int(x.get("tris", 0)) for x in man.get("result", {}).get("rows", []))
        out.append(lv)
    out.sort(key=lambda x: -(x["cell"] or 0))
    return out


def check(levels, min_iou=0.35, ratio=4):
    """返回 `(passed, reasons)`。"""
    reasons = [lv["why"] for lv in levels if lv.get("why")]
    if len(levels) < 2:
        reasons.append("至少需要两级")
        return False, reasons
    if any(lv.get("status") != "verified" for lv in levels):
        reasons.append("有级别未通过独立复核")
    if any(lv.get("iou") is None for lv in levels):
        reasons.append("有级别缺 3D IoU")
    if any(lv.get("tris") is None for lv in levels):
        reasons.append("有级别缺三角面数")
    coarse, fine = levels[0], levels[-1]
    if coarse.get("iou") is not None and coarse["iou"] < min_iou:
        reasons.append(f"最粗一级 IoU {coarse['iou']:.4f} < {min_iou}")
    if coarse.get("tris") is not None and fine.get("tris"):
        if coarse["tris"] * ratio > fine["tris"]:
            reasons.append(
                f"最粗一级三角面 {coarse['tris']} > 最细 {fine['tris']}/{ratio}"
                f"（={fine['tris'] / ratio:.0f}）"
            )
    return not reasons, reasons


def report(levels, min_iou, ratio):
    print(f"级别（粗 → 细），门：IoU≥{min_iou}，最粗三角面 ≤ 最细/{ratio}")
    fine = levels[-1]["tris"] if levels and levels[-1].get("tris") else None
    for lv in levels:
        cell = "?" if lv.get("cell") is None else f"{lv['cell']:g}"
        iou = "?" if lv.get("iou") is None else f"{lv['iou']:.4f}"
        tris = "?" if lv.get("tris") is None else str(lv["tris"])
        share = ""
        if fine and lv.get("tris") is not None:
            share = f"  体积比 {lv['tris'] / fine:.3f}"
        print(f"  cell {cell:>5} m  IoU {iou:>7}  tris {tris:>9}{share}")
    passed, reasons = check(levels, min_iou, ratio)
    if passed:
        print("RESULT: PASS")
    else:
        print("RESULT: FAIL")
        for r in reasons:
            print(f"  - {r}")
    return 0 if passed else 1


def self_test():
    import tempfile

    def build(root, rows):
        """rows: [(cell, iou, tris, status)]；写清单与 sweep.json。"""
        sweep = {"rows": []}
        for row in rows:
            cell, iou, tris = row[0], row[1], row[2]
            status = row[3] if len(row) > 3 else "verified"
            model = root / f"cell{cell}.occ1"
            model.write_bytes(b"OCC1 1 1\n")
            (root / f"cell{cell}.occ1.manifest.json").write_text(
                json.dumps({"result": {"rows": [{"tris": tris}]}}), encoding="utf-8"
            )
            sweep["rows"].append(
                {"config": {"cell": cell}, "model": str(model), "status": status,
                 "metrics": {"iou_mean": iou}}
            )
        p = root / "sweep.json"
        p.write_text(json.dumps(sweep), encoding="utf-8")
        return p

    cases = []
    with tempfile.TemporaryDirectory() as td:
        root = pathlib.Path(td)
        p = build(root, [(0.04, 0.64, 86), (0.02, 0.51, 416), (0.01, 0.70, 1468)])
        lv = levels_from(p)
        assert [x["cell"] for x in lv] == [0.04, 0.02, 0.01], "应按粗→细"
        cases.append(("通过", check(lv)[0], True))
        cases.append(("体积更低更好", check(lv)[0], True))
        p = build(root, [(0.04, 0.30, 86), (0.01, 0.70, 1468)])
        cases.append(("粗级 IoU 不够", check(levels_from(p))[0], False))
        p = build(root, [(0.04, 0.64, 400), (0.01, 0.70, 1468)])
        cases.append(("粗级体积超 1/4", check(levels_from(p))[0], False))
        p = build(root, [(0.04, 0.64, 86, "verify-failed"), (0.01, 0.70, 1468)])
        cases.append(("有未复核级别", check(levels_from(p))[0], False))
        p = build(root, [(0.01, 0.70, 1468)])
        cases.append(("只有一级", check(levels_from(p))[0], False))
        # 缺清单
        sweep = {"rows": [{"config": {"cell": 0.04}, "model": str(root / "nope.occ1"),
                           "status": "verified", "metrics": {"iou_mean": 0.6}},
                          {"config": {"cell": 0.01}, "model": str(root / "cell0.01.occ1"),
                           "status": "verified", "metrics": {"iou_mean": 0.7}}]}
        p = root / "s2.json"
        p.write_text(json.dumps(sweep), encoding="utf-8")
        cases.append(("缺清单判红", check(levels_from(p))[0], False))
        # 阈值可调
        p = build(root, [(0.04, 0.40, 86), (0.01, 0.70, 1468)])
        cases.append(("放宽到 0.40", check(levels_from(p), min_iou=0.40)[0], True))
        cases.append(("收紧到 0.50", check(levels_from(p), min_iou=0.50)[0], False))
        p = build(root, [(0.04, 0.64, 200), (0.01, 0.70, 1468)])
        cases.append(("比例 4 通过", check(levels_from(p))[0], True))
        cases.append(("比例收紧到 8 失败", check(levels_from(p), ratio=8)[0], False))
    bad = [n for n, got, want in cases if got != want]
    for n, got, want in cases:
        print(f"  {n}: {'ok' if got == want else 'FAIL'}（got={got} want={want}）")
    if bad:
        print(f"FAIL {len(bad)} 条：{bad}")
        sys.exit(1)
    print(f"self-test {len(cases)}/{len(cases)} 通过")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--sweep")
    ap.add_argument("--min-iou", type=float, default=0.35)
    ap.add_argument("--ratio", type=float, default=4.0)
    ap.add_argument("--self-test", action="store_true")
    args = ap.parse_args()
    if args.self_test:
        self_test()
        return
    if not args.sweep:
        ap.error("需要 --sweep（或 --self-test）")
    sys.exit(report(levels_from(args.sweep), args.min_iou, args.ratio))


if __name__ == "__main__":
    main()
