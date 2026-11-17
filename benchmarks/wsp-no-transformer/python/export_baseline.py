"""把论文仓库里的一个模型导出成 Rust 复现所需的 npz（对齐基线，不参与交付）。

用法：
    python export_baseline.py <paper_repo>/models/opus/mem_assoc2_g5-arc_blind_K0_r0 <out_dir>

产物：
    weights.npz  concept / idf / A / row_ids / row_w / mix（与 build.build() 的返回逐值相同）
    arc_test.npz q / o / y / subset（ARC-Easy + ARC-Challenge test，四选项题）
"""
import importlib.util
import sys
from pathlib import Path

import numpy as np

from wsp import arc, free_model


def load_module(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    mod = importlib.util.module_from_spec(spec)
    sys.modules[name] = mod
    spec.loader.exec_module(mod)
    return mod


def main() -> None:
    folder = Path(sys.argv[1]).resolve()
    out = Path(sys.argv[2]).resolve()
    sys.path.insert(0, str(folder))
    out.mkdir(parents=True, exist_ok=True)

    build = load_module("model_build", folder / "build.py")
    w = build.build()
    a = w["A"]
    if hasattr(a, "to_dense"):
        a = a.to_dense()
    arrays = {
        "concept": w["concept"].numpy(),
        "idf": w["idf"].numpy(),
        "A": a.numpy(),
        "row_ids": w["row_ids"].numpy(),
        "row_w": w["row_w"].numpy(),
        "mix": np.asarray(w["mix"].reshape(-1).numpy(), dtype=np.float32),
    }
    np.savez_compressed(out / "weights.npz", **arrays)

    vocab = free_model.vocab_of(folder)
    q, o, y = arc.load("test")
    qi, oi = free_model.encode(vocab, q, o)
    np.savez_compressed(
        out / "arc_test.npz",
        q=qi,
        o=oi,
        y=y,
        subset=np.array(arc.subset("test"), dtype=str),
    )

    print(
        {
            "model": folder.name,
            "concept": arrays["concept"].shape,
            "idf": arrays["idf"].shape,
            "A": arrays["A"].shape,
            "row_ids": arrays["row_ids"].shape,
            "row_w": arrays["row_w"].shape,
            "mix": float(arrays["mix"][0]),
            "arc_test": qi.shape,
        }
    )


if __name__ == "__main__":
    main()
