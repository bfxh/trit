"""Evaluate WSPEMB1 embedding runs on SciFact retrieval.

Reads ``texts.json`` + ``qrels.json`` produced by ``wsp_export_scifact.py`` and
one or more ``WSPEMB1`` files.  Reports Recall@k, nDCG@k and MRR@10, with an
optional query-level bootstrap of candidate-vs-baseline deltas.

Example:
  python scripts/wsp_retrieval_eval.py \
    --texts /path/scifact-l12/texts.json --qrels /path/scifact-l12/qrels.json \
    --run f32=/path/emb/l12-pure-f32.f32 \
    --run int3=/path/emb/l12-int3.f32 \
    --baseline f32 --baseline int3 --bootstrap 10000 \
    --out /path/scifact-l12/retrieval.json
"""

import argparse
import json
import struct
import sys
from pathlib import Path

import numpy as np

DEFAULT_KS = "10,100"
DEFAULT_BOOTSTRAP = 10000


def read_emb(path):
    with open(path, "rb") as handle:
        magic = handle.read(8)
        if magic != b"WSPEMB1\0":
            raise ValueError(f"{path}: bad magic {magic!r}")
        rows, cols = struct.unpack("<II", handle.read(8))
        data = np.frombuffer(handle.read(), dtype="<f4")
    if data.size != rows * cols:
        raise ValueError(f"{path}: {data.size} floats != {rows}x{cols}")
    return data.reshape(rows, cols)


def normalize(matrix):
    norms = np.linalg.norm(matrix, axis=1, keepdims=True)
    norms[norms == 0] = 1.0
    return matrix / norms


def rank_metrics(scores, relevant, corpus_ids, ks):
    max_k = min(max(ks), len(corpus_ids))
    if max_k <= 0:
        return {f"recall@{k}": 0.0 for k in ks} | {f"ndcg@{k}": 0.0 for k in ks} | {"mrr@10": 0.0}
    top = np.argpartition(-scores, max_k - 1)[:max_k]
    top = top[np.argsort(-scores[top], kind="stable")]
    ranked = [corpus_ids[index] for index in top]
    out = {}
    for k in ks:
        kk = min(k, len(ranked))
        hits = [1 if doc in relevant else 0 for doc in ranked[:kk]]
        out[f"recall@{k}"] = sum(hits) / len(relevant) if relevant else 0.0
        dcg = sum(1.0 / np.log2(rank + 1) for rank, hit in enumerate(hits, start=1) if hit)
        ideal = sum(1.0 / np.log2(rank + 1) for rank in range(1, min(len(relevant), kk) + 1))
        out[f"ndcg@{k}"] = dcg / ideal if ideal > 0 else 0.0
    mrr_k = min(10, max_k)
    first = next((rank for rank, doc in enumerate(ranked[:mrr_k], start=1) if doc in relevant), None)
    out["mrr@10"] = 1.0 / first if first else 0.0
    return out


def parse_args():
    parser = argparse.ArgumentParser()
    parser.add_argument("--texts", required=True)
    parser.add_argument("--qrels", required=True)
    parser.add_argument("--run", action="append", default=[], metavar="NAME=PATH")
    parser.add_argument("--baseline", action="append", default=[])
    parser.add_argument("--ks", default=DEFAULT_KS)
    parser.add_argument("--bootstrap", type=int, default=DEFAULT_BOOTSTRAP)
    parser.add_argument("--seed", type=int, default=7)
    parser.add_argument("--out", default="")
    return parser.parse_args()


def main():
    args = parse_args()
    ks = sorted({int(item.strip()) for item in args.ks.split(",") if item.strip()})
    if not ks:
        raise SystemExit("--ks is empty")
    texts = json.loads(Path(args.texts).read_text(encoding="utf-8"))["texts"]
    qrels = json.loads(Path(args.qrels).read_text(encoding="utf-8"))
    corpus_rows = [row for row in texts if row["kind"] == "corpus"]
    query_rows = [row for row in texts if row["kind"] == "query"]
    corpus_ids = [row["id"] for row in corpus_rows]
    corpus_index = [row["index"] for row in corpus_rows]
    query_ids = [row["id"] for row in query_rows]
    query_index = [row["index"] for row in query_rows]
    eligible = [qid for qid in query_ids if qrels.get(qid)]
    if not eligible:
        raise SystemExit("no qrels queries present in texts.json")

    runs = []
    for item in args.run:
        if "=" not in item:
            raise SystemExit(f"--run must be NAME=PATH, got {item!r}")
        name, path = item.split("=", 1)
        emb = read_emb(path)
        if emb.shape[0] != len(texts):
            raise SystemExit(f"{name}: rows {emb.shape[0]} != texts {len(texts)}")
        runs.append((name, normalize(emb)))

    query_pos = {qid: pos for pos, qid in enumerate(query_ids)}
    metrics = ["mrr@10"] + [f"recall@{k}" for k in ks] + [f"ndcg@{k}" for k in ks]
    results = {}
    per_query = {}
    for name, emb in runs:
        q = emb[query_index]
        c = emb[corpus_index]
        scores = q @ c.T
        rows = {metric: [] for metric in metrics}
        for qid in eligible:
            pos = query_pos[qid]
            relevant = set(qrels[qid])
            values = rank_metrics(scores[pos], relevant, corpus_ids, ks)
            for metric in metrics:
                rows[metric].append(values[metric])
        results[name] = {
            "queries": len(eligible),
            "metrics": {metric: float(np.mean(values)) if values else 0.0 for metric, values in rows.items()},
            "shape": [int(emb.shape[0]), int(emb.shape[1])],
        }
        per_query[name] = rows

    print(f"queries={len(eligible)} corpus={len(corpus_ids)}")
    print("name\t" + "\t".join(metrics))
    for name, _ in runs:
        values = results[name]["metrics"]
        print(name + "\t" + "\t".join(f"{values[metric]:.6f}" for metric in metrics))

    boot = {}
    if args.bootstrap > 0 and args.baseline:
        rng = np.random.default_rng(args.seed)
        n = len(eligible)
        indices = rng.integers(0, n, size=(args.bootstrap, n))
        for candidate, _ in runs:
            if candidate in args.baseline:
                continue
            for baseline in args.baseline:
                if baseline == candidate or baseline not in per_query:
                    continue
                for metric in metrics:
                    cand = np.asarray(per_query[candidate][metric], dtype=float)
                    base = np.asarray(per_query[baseline][metric], dtype=float)
                    deltas = cand[indices].mean(axis=1) - base[indices].mean(axis=1)
                    boot[f"{candidate}_vs_{baseline}:{metric}"] = {
                        "mean_delta": float(deltas.mean()),
                        "ci_low": float(np.quantile(deltas, 0.025)),
                        "ci_high": float(np.quantile(deltas, 0.975)),
                        "better_fraction": float(np.mean(deltas > 0)),
                        "queries": n,
                        "iters": args.bootstrap,
                        "seed": args.seed,
                    }

    payload = {
        "texts": args.texts,
        "qrels": args.qrels,
        "ks": ks,
        "runs": results,
        "bootstrap": boot,
    }
    if args.out:
        Path(args.out).write_text(json.dumps(payload, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
        print(f"wrote {args.out}")


if __name__ == "__main__":
    sys.exit(main())
