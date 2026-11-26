"""把 6 个 `matched-prior-*.json` 合并成 360/72/36/24 行 TSV。

环境变量：
  TRIT_RESULTS   默认 `./trit-results`
  TRIT_GEOM_OUT  默认 `<TRIT_RESULTS>/geometry/corrected`
  TRIT_DATA_DIR  默认仓库 `docs/research/data`

用法：`python docs/research/scripts/wsp_m138/make_matched_prior_tsv.py`
"""

import csv
import json
import os
from pathlib import Path

import numpy as np

RESULTS = Path(os.environ.get("TRIT_RESULTS", "trit-results"))
GEOM = Path(os.environ.get("TRIT_GEOM_OUT", str(RESULTS / "geometry" / "corrected")))
REPO_ROOT = Path(__file__).resolve().parents[4]
REPO = Path(os.environ.get("TRIT_DATA_DIR", str(REPO_ROOT / "docs" / "research" / "data")))
CASES = [("sst2", "l12"), ("sst2", "l6"), ("cola", "l12"), ("cola", "l6"), ("hate", "l12"), ("hate", "l6")]
RULES = ("default", "target_prior")


def main():
    raw = []
    for dataset, depth in CASES:
        p = GEOM / f"matched-prior-{dataset}-{depth}.json"
        raw.extend(json.loads(p.read_text(encoding="utf-8"))["rows"])
    raw.sort(key=lambda r: (r["dataset"], r["depth"], r["probe"], r["arm"], r["rule"], r["seed"]))

    fields = ["dataset", "depth", "probe", "arm", "metric", "seed", "rule", "C", "threshold", "target_prior",
              "pred_pos_rate", "f32_score", "arm_score", "point_delta", "boot_mean", "ci_low", "ci_high", "verdict"]
    with (REPO / "wsp-classify-matched-prior-raw.tsv").open("w", encoding="utf-8", newline="") as f:
        w = csv.DictWriter(f, fieldnames=fields, delimiter="\t", lineterminator="\n")
        w.writeheader()
        for r in raw:
            w.writerow({k: (f"{r[k]:.6f}" if isinstance(r[k], float) else r[k]) for k in fields})

    summary = []
    groups = {}
    for r in raw:
        groups.setdefault((r["dataset"], r["depth"], r["probe"], r["arm"], r["metric"], r["rule"]), []).append(r)
    for key, rows in sorted(groups.items()):
        vals = np.array([r["point_delta"] for r in rows], dtype=float)
        boots = np.array([r["boot_mean"] for r in rows], dtype=float)
        verdicts = [r["verdict"] for r in rows]
        summary.append({
            "dataset": key[0], "depth": key[1], "probe": key[2], "arm": key[3], "metric": key[4], "rule": key[5],
            "n_seeds": len(rows), "n_pos_ci": verdicts.count("positive"), "n_neg_ci": verdicts.count("negative"),
            "n_cross_ci": verdicts.count("cross"), "n_pos_point": int((vals > 0).sum()),
            "any_positive": int(verdicts.count("positive") > 0),
            "mean_point": f"{vals.mean():.6f}", "sd_point": f"{vals.std(ddof=1):.6f}" if len(vals) > 1 else "0.000000",
            "min_point": f"{vals.min():.6f}", "max_point": f"{vals.max():.6f}", "mean_boot": f"{boots.mean():.6f}",
            "mean_threshold": f"{np.mean([r['threshold'] for r in rows]):.6f}",
        })

    with (REPO / "wsp-classify-matched-prior-summary.tsv").open("w", encoding="utf-8", newline="") as f:
        w = csv.DictWriter(f, fieldnames=list(summary[0].keys()), delimiter="\t", lineterminator="\n")
        w.writeheader(); w.writerows(summary)

    # 36 comparison-level default vs target-prior, aggregated over seeds
    cmp_rows = []
    by_comp = {}
    for r in raw:
        by_comp.setdefault((r["dataset"], r["depth"], r["probe"], r["arm"], r["metric"]), {}).setdefault(r["rule"], []).append(r)
    for key, rules in sorted(by_comp.items()):
        d_rows, t_rows = rules["default"], rules["target_prior"]
        def agg(rows):
            vs = [r["verdict"] for r in rows]
            return {
                "n_pos": vs.count("positive"), "n_neg": vs.count("negative"), "n_cross": vs.count("cross"),
                "any_positive": int(vs.count("positive") > 0), "mean_point": float(np.mean([r["point_delta"] for r in rows])),
            }
        d, t = agg(d_rows), agg(t_rows)
        cmp_rows.append({
            "dataset": key[0], "depth": key[1], "probe": key[2], "arm": key[3], "metric": key[4],
            "default_n_pos": d["n_pos"], "default_n_neg": d["n_neg"], "default_n_cross": d["n_cross"],
            "default_any_positive": d["any_positive"], "default_mean_point": f"{d['mean_point']:.6f}",
            "target_n_pos": t["n_pos"], "target_n_neg": t["n_neg"], "target_n_cross": t["n_cross"],
            "target_any_positive": t["any_positive"], "target_mean_point": f"{t['mean_point']:.6f}",
            "flip": "gain" if d["any_positive"] == 0 and t["any_positive"] == 1 else
                    "loss" if d["any_positive"] == 1 and t["any_positive"] == 0 else "same",
        })
    with (REPO / "wsp-classify-matched-prior-vs-default.tsv").open("w", encoding="utf-8", newline="") as f:
        w = csv.DictWriter(f, fieldnames=list(cmp_rows[0].keys()), delimiter="\t", lineterminator="\n")
        w.writeheader(); w.writerows(cmp_rows)

    # f32 absolute under both rules
    f32 = []
    g = {}
    for r in raw:
        g.setdefault((r["dataset"], r["depth"], r["probe"], r["rule"]), {})[r["seed"]] = r["f32_score"]
    for key, by_seed in sorted(g.items()):
        vals = list(by_seed.values())
        f32.append({"dataset": key[0], "depth": key[1], "probe": key[2], "rule": key[3], "n_seeds": len(vals),
                    "mean_score": f"{np.mean(vals):.6f}", "min_score": f"{np.min(vals):.6f}", "max_score": f"{np.max(vals):.6f}"})
    with (REPO / "wsp-classify-matched-prior-f32.tsv").open("w", encoding="utf-8", newline="") as f:
        w = csv.DictWriter(f, fieldnames=list(f32[0].keys()), delimiter="\t", lineterminator="\n")
        w.writeheader(); w.writerows(f32)

    for rule in RULES:
        rs = [s for s in summary if s["rule"] == rule]
        n_pos = sum(s["n_pos_ci"] for s in rs)
        n_neg = sum(s["n_neg_ci"] for s in rs)
        n_cross = sum(s["n_cross_ci"] for s in rs)
        comps_pos = sum(1 for s in rs if s["any_positive"])
        print(rule, "CI", n_pos, n_neg, n_cross, "comparisons with positive", comps_pos, "of", len(rs), flush=True)
        for s in rs:
            if s["n_pos_ci"] > 0:
                print("POS", s, flush=True)
    print("raw", len(raw), "summary", len(summary), "cmp", len(cmp_rows), "f32", len(f32), flush=True)


if __name__ == "__main__":
    main()
