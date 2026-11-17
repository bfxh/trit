"""M0a 的独立 oracle：用 Python 重写同一份 2D 网格生成与 BFS，与 Rust 输出逐行对拍。

只作对齐基线，不参与交付（trit 规则：Python 不交付）。

用法：
    python python/bfs_ref.py --tasks 1000 --out ref.jsonl
    python python/bfs_ref.py --check dataset.jsonl
"""

import argparse
import json
import sys
from collections import deque
from pathlib import Path

N = 8
START = 0
MASK = (1 << 64) - 1


def xorshift(state: int) -> int:
    x = state
    x ^= (x << 13) & MASK
    x ^= x >> 7
    x ^= (x << 17) & MASK
    return x & MASK


def neighbors(i: int):
    r, c = divmod(i, N)
    if r > 0:
        yield i - N
    if c + 1 < N:
        yield i + 1
    if r + 1 < N:
        yield i + N
    if c > 0:
        yield i - 1


def bfs_dist(grid, start: int):
    dist = [None] * (N * N)
    dist[start] = 0
    q = deque([start])
    while q:
        i = q.popleft()
        for j in neighbors(i):
            if grid[j] == 0 and dist[j] is None:
                dist[j] = dist[i] + 1
                q.append(j)
    return dist


def bfs_path(grid, start: int, goal: int):
    if grid[start] != 0 or grid[goal] != 0:
        return None
    prev = [None] * (N * N)
    seen = [False] * (N * N)
    seen[start] = True
    q = deque([start])
    while q:
        i = q.popleft()
        if i == goal:
            break
        for j in neighbors(i):
            if grid[j] == 0 and not seen[j]:
                seen[j] = True
                prev[j] = i
                q.append(j)
    if not seen[goal]:
        return None
    path = [goal]
    cur = goal
    while cur != start:
        cur = prev[cur]
        path.append(cur)
    path.reverse()
    return path


def gen_task(task_id: int, state: int):
    grid = [0] * (N * N)
    for i in range(N * N):
        state = xorshift(state)
        if state % 100 < 20:
            grid[i] = 1
    grid[START] = 0
    dist = bfs_dist(grid, START)
    reachable = [i for i, d in enumerate(dist) if d is not None]
    goal = min(reachable, key=lambda i: (-dist[i], i))
    grid[goal] = 0
    path = bfs_path(grid, START, goal) or [START]
    return {
        "id": task_id,
        "grid": "".join("." if v == 0 else "#" for v in grid),
        "start": START,
        "goal": goal,
        "len": len(path),
        "path": path,
    }, state


def generate(tasks: int, seed: int):
    state = seed
    for task_id in range(tasks):
        task, state = gen_task(task_id, state)
        yield task


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--tasks", type=int, default=1000)
    ap.add_argument("--seed", type=lambda v: int(v, 0), default=0x9E37_79B9_7F4A_7C15)
    ap.add_argument("--out", type=Path)
    ap.add_argument("--check", type=Path)
    args = ap.parse_args()

    if args.check:
        rows = [json.loads(line) for line in args.check.read_text(encoding="utf-8").splitlines()]
        ref = list(generate(len(rows), args.seed))
        diff = sum(1 for a, b in zip(rows, ref) if a != b)
        print(f"行数 {len(rows)}｜DIFF {diff}")
        if diff:
            for i, (a, b) in enumerate(zip(rows, ref)):
                if a != b:
                    print(f"首条差异行 {i}:\n rust={a}\n ref ={b}")
                    break
        return 0 if diff == 0 else 1

    lines = [json.dumps(t, separators=(",", ":")) for t in generate(args.tasks, args.seed)]
    if args.out:
        args.out.write_text("\n".join(lines) + "\n", encoding="utf-8")
    else:
        sys.stdout.write("\n".join(lines) + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
