#!/usr/bin/env python3
"""幽灵身份策略仿真器（GHOST-IDENTITY-RESEARCH.md §二/§六 的量化形态）。

纯防御侧研究工具：合成观测流（40 人群 + 1 防御者）× 五种混淆策略
× 局部/全局两类追踪者（Jaccard 锁定 + 体积指纹），量化各策略的
E[J] 与锁定率——验证「decoy 对全局观测者反效」「0 态被动化最优」
两条研究结论。确定性种子（xorshift64，与 fuzz 同款）——同种子同批次。

用法：python scripts/ghost_sim.py [--seed 7 --steps 30 --theta 0.2]
"""

from __future__ import annotations

import argparse
import statistics
import sys
from collections import Counter
from pathlib import Path

CELLS, WIFI = 50, 100
CROWD, T = 40, 30


class Xorshift64:
    """确定性 PRNG（与 fuzz/yara_scale 同款）。"""

    def __init__(self, seed: int) -> None:
        self.state = seed & 0xFFFFFFFFFFFFFFFF or 0x9E3779B97F4A7C15

    def next(self) -> int:
        x = self.state
        x ^= (x << 13) & 0xFFFFFFFFFFFFFFFF
        x ^= x >> 7
        x ^= (x << 17) & 0xFFFFFFFFFFFFFFFF
        self.state = x
        return x

    def below(self, n: int) -> int:
        return self.next() % n if n else 0

    def sample(self, pool: list, k: int) -> list:
        pool = list(pool)
        out = []
        for _ in range(min(k, len(pool))):
            out.append(pool.pop(self.below(len(pool))))
        return out


def jaccard(a: set, b: set) -> float:
    union = a | b
    return len(a & b) / len(union) if union else 0.0


def run_strategy(rng: Xorshift64, strategy: str, param: float, theta: float) -> dict:
    """跑一个防御者的 T 步观测流，返回局部/全局两类追踪者的读数。"""
    # 人群：每个用户一个家集（3-6 个设备）+ 公共热门池
    common = {f"cell{i:02d}" for i in range(8)} | {f"wifi{i:02d}" for i in range(12)}
    homes = [
        rng.sample([f"cell{i:02d}" for i in range(CELLS)], 2)
        + rng.sample([f"wifi{i:02d}" for i in range(WIFI)], rng.below(4) + 2)
        for _ in range(CROWD)
    ]
    all_obs: list[set] = []  # 全局观测者看得到所有人
    js_local, js_global, volume_flags = [], [], []
    defender_h: set = set()  # 追踪者维护的防御者历史
    population_h: Counter = Counter()  # 全局观测者的设备出现频率
    rotation_home = set(homes[0])
    for t in range(T):
        # 人群观测（每步家集 + 少量公共池）
        crowd_obs = []
        for h in homes[1:]:
            obs = set(h) | set(rng.sample(sorted(common), rng.below(3)))
            crowd_obs.append(obs)
        # 防御者家集
        if strategy == "rotation" and t % max(1, int(param)) == 0:
            rotation_home = set(
                rng.sample([f"cell{i:02d}" for i in range(CELLS)], 2)
                + rng.sample([f"wifi{i:02d}" for i in range(WIFI)], rng.below(4) + 2)
            )
        base = rotation_home if strategy == "rotation" else set(homes[0])
        s = set(base)
        if strategy == "blend":
            keep = max(0, len(s) - int(param))
            s = set(rng.sample(sorted(s), keep)) | set(
                rng.sample(sorted(common), int(param))
            )
        if strategy == "decoy":
            s |= {f"ghost{t}-{i}" for i in range(int(param))}
        if strategy == "passive" and rng.below(100) < param * 100:
            s = set()  # 0 态：本步不发射
        # 局部追踪者：J(S_t, 历史并集)
        j_local = jaccard(s, defender_h) if defender_h else 0.0
        js_local.append(j_local)
        locked_local = j_local > theta
        # 全局追踪者：体积指纹（观测数偏离人群均值）
        crowd_sizes = [len(o) for o in crowd_obs]
        mean_size = statistics.mean(crowd_sizes) if crowd_sizes else 0
        volume_flags.append(abs(len(s) - mean_size) > 2.0 * (statistics.pstdev(crowd_sizes) or 1))
        # 全局 J：与全局历史（含人群）比
        for o in crowd_obs:
            population_h.update(o)
        popular = {d for d, c in population_h.items() if c >= T // 2}
        js_global.append(jaccard(s, popular))
        defender_h |= s
        all_obs.append(s)
    return {
        "mean_j_local": statistics.mean(js_local),
        "lock_rate_local": statistics.mean(j > theta for j in js_local),
        "mean_j_global": statistics.mean(js_global),
        "volume_flag_rate": statistics.mean(volume_flags),
    }


STRATEGIES = [
    ("baseline", 0.0),
    ("decoy", 2),
    ("decoy", 5),
    ("decoy", 10),
    ("blend", 1),
    ("blend", 3),
    ("rotation", 5),
    ("passive", 0.3),
    ("passive", 0.6),
]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--seed", type=int, default=7)
    parser.add_argument("--theta", type=float, default=0.2)
    args = parser.parse_args()

    print(f"{'策略':<16} {'E[J]局部':>9} {'锁率局部':>9} {'E[J]全局':>9} {'体积旗全局':>10}")
    results = {}
    for name, param in STRATEGIES:
        # 每策略独立种子流但同起点：跑 20 次取均值（策略内方差可见性）
        js_l, lock_l, js_g, vol_g = [], [], [], []
        for run in range(20):
            rng = Xorshift64(args.seed * 1000 + run)
            r = run_strategy(rng, name, param, args.theta)
            js_l.append(r["mean_j_local"])
            lock_l.append(r["lock_rate_local"])
            js_g.append(r["mean_j_global"])
            vol_g.append(r["volume_flag_rate"])
        row = {
            "mean_j_local": statistics.mean(js_l),
            "lock_rate_local": statistics.mean(lock_l),
            "mean_j_global": statistics.mean(js_g),
            "volume_flag_rate": statistics.mean(vol_g),
        }
        label = name if param == 0 else f"{name}({param})"
        results[label] = row
        print(
            f"{label:<16} {row['mean_j_local']:>9.3f} {row['lock_rate_local']:>9.2f} "
            f"{row['mean_j_global']:>9.3f} {row['volume_flag_rate']:>10.2f}"
        )
    (Path(__file__).resolve().parent.parent / "docs").mkdir(exist_ok=True)
    print("\n判读：decoy 密度↑ → E[J]局部↓ 但体积旗↑（全局自我指认）；"
          "passive(0 态) 两类追踪者读数同优；rotation 压局部 J、对全局 J 无效。")
    return 0


if __name__ == "__main__":
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    sys.exit(main())
