"""跑一遍论文模型并保存 logits，供 Rust 侧逐题对拍（对齐基线，不参与交付）。

用法：
    python dump_logits.py <paper_repo>/models/opus/mem_assoc2_g5-arc_blind_K0_r0 <out_dir>
"""
import importlib.util
import json
import sys
from pathlib import Path

import numpy as np
import torch

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
    out.mkdir(parents=True, exist_ok=True)

    vocab = free_model.vocab_of(folder)
    q, o, y = arc.load("test")
    qi, oi = free_model.encode(vocab, q, o)
    build = load_module("model_build", folder / "build.py")
    arch = load_module("model_arch", folder / "arch.py")
    w = build.build()
    with torch.no_grad():
        logits = torch.cat(
            [
                arch.forward(
                    w,
                    torch.from_numpy(qi[i : i + 64]),
                    torch.from_numpy(oi[i : i + 64]),
                ).float()
                for i in range(0, len(qi), 64)
            ]
        ).numpy()
    np.save(out / "logits.npy", logits)
    np.save(out / "pred.npy", logits.argmax(1))
    np.save(out / "y.npy", y)
    res = free_model.accuracy(logits, y)
    print(json.dumps(res))


if __name__ == "__main__":
    main()
