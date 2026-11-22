#!/usr/bin/env python3
"""延迟分位（p50/p95/p99/p999）+ 写放大。

口径：1 连接、单进程、每档 `--queries` 次查询、热态（服务已起）。
用法：
    python scripts/perf/matrix.py --sizes 1000,10000 --queries 50000
"""

from __future__ import annotations

import argparse
import pathlib
import sys
import tempfile
import time

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import pg_client as pg


def measure(bin_path: str, wal: pathlib.Path, size: int, queries: int, port: int) -> dict:
    pg.prepare_wal(bin_path, wal, events=size, sync=True)
    db = wal.with_suffix(".sqlite")
    dwwal = pathlib.Path(str(db) + "-wal")

    logical = sum(len(str(pg.event(i)).encode()) for i in range(1, size + 1))
    wal_bytes = wal.stat().st_size if wal.exists() else 0
    db_bytes = db.stat().st_size if db.exists() else 0
    dwwal_bytes = dwwal.stat().st_size if dwwal.exists() else 0

    serve = pg.start_server(bin_path, wal, f"127.0.0.1:{port}")
    try:
        sock = pg.connect(port)
        lat = []
        for _ in range(queries):
            elapsed, _ = pg.one_query(sock, "SELECT count(*) AS n FROM trit_trace")
            lat.append(elapsed)
        sock.close()
    finally:
        pg.stop_server(serve)
        pg.cleanup(wal)

    total = sum(lat)
    return {
        "size": size,
        "queries": len(lat),
        "qps": len(lat) / total if total else 0,
        "p50": pg.percentile(lat, 0.50) * 1000,
        "p95": pg.percentile(lat, 0.95) * 1000,
        "p99": pg.percentile(lat, 0.99) * 1000,
        "p999": pg.percentile(lat, 0.999) * 1000,
        "max": max(lat) * 1000,
        "logical": logical,
        "wal": wal_bytes,
        "sqlite": db_bytes + dwwal_bytes,
        "amp_wal": wal_bytes / logical if logical else 0,
        "amp_sqlite": (db_bytes + dwwal_bytes) / logical if logical else 0,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    pg.add_common_args(parser)
    parser.add_argument("--sizes", default="1000,10000", help="事件规模，逗号分隔")
    parser.add_argument("--queries", type=int, default=50_000, help="每档查询次数")
    args = parser.parse_args()

    wal = pathlib.Path(tempfile.gettempdir()) / f"perf-matrix-{time.time_ns()}.wal"
    sizes = [int(value) for value in args.sizes.split(",") if value]
    print(
        f"{'size':>7} {'queries':>8} {'qps':>8} {'p50':>7} {'p95':>7} {'p99':>7} "
        f"{'p999':>8} {'max':>8} {'logical':>10} {'wal':>10} {'sqlite':>10} "
        f"{'amp_wal':>8} {'amp_sqlite':>10}"
    )
    for size in sizes:
        result = measure(args.bin, wal, size, args.queries, args.port)
        print(
            f"{result['size']:>7} {result['queries']:>8} {result['qps']:>8.0f} "
            f"{result['p50']:>7.3f} {result['p95']:>7.3f} {result['p99']:>7.3f} "
            f"{result['p999']:>8.3f} {result['max']:>8.2f} "
            f"{result['logical']:>10} {result['wal']:>10} {result['sqlite']:>10} "
            f"{result['amp_wal']:>8.2f} {result['amp_sqlite']:>10.2f}"
        )
    return 0


if __name__ == "__main__":
    sys.exit(main())
