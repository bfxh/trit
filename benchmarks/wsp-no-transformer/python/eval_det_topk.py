"""确定性 tie-breaking 的参考实现：与 Rust 侧相同的 top-16 规则（值降序、索引升序）。

用来证明 Rust 与论文的 3 题差异只来自 torch.topk 对并列 idf（0.8）的未定义顺序。
"""
import importlib.util
import json
import sys
from pathlib import Path

import numpy as np
import torch

from wsp import arc, free_model


def load_module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    mod = importlib.util.module_from_spec(spec)
    sys.modules[name] = mod
    spec.loader.exec_module(mod)
    return mod


J, PRIME = 16, 1_000_003


def topk_det(qb, j):
    arr = qb.numpy()
    order = np.argsort(-arr, kind="stable")[:, :j]
    vals = np.take_along_axis(arr, order, axis=1)
    return torch.from_numpy(vals), torch.from_numpy(order)


def forward_det(arch, w, q, o):
    n, C = q.shape[0], w["idf"].shape[0]
    qb, ob = arch.bags(w, q, o)
    vals, idx = topk_det(qb, J)
    a, b = torch.triu_indices(J, J, 1)
    lo = torch.minimum(idx[:, a], idx[:, b])
    hi = torch.maximum(idx[:, a], idx[:, b])
    rows = (lo * PRIME + hi) % w["row_ids"].shape[0]
    strength = vals[:, a] * vals[:, b]
    read = torch.zeros(n, C).scatter_add_(
        1,
        w["row_ids"][rows].reshape(n, -1),
        (strength[:, :, None] * w["row_w"][rows]).reshape(n, -1),
    )
    read = (read + w["mix"] * (qb @ w["A"])) * (qb == 0).float()
    return torch.einsum("nc,nkc->nk", read, ob)


def main() -> None:
    folder = Path(sys.argv[1]).resolve()
    vocab = free_model.vocab_of(folder)
    q, o, y = arc.load("test")
    qi, oi = free_model.encode(vocab, q, o)
    build = load_module("model_build", folder / "build.py")
    arch = load_module("model_arch", folder / "arch.py")
    w = build.build()
    with torch.no_grad():
        logits = torch.cat(
            [
                forward_det(
                    arch,
                    w,
                    torch.from_numpy(qi[i : i + 64]),
                    torch.from_numpy(oi[i : i + 64]),
                ).float()
                for i in range(0, len(qi), 64)
            ]
        ).numpy()
    np.save(Path(sys.argv[2]) / "logits.det.npy", logits)
    print(json.dumps(free_model.accuracy(logits, y)))


if __name__ == "__main__":
    main()
