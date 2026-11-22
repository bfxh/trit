#!/usr/bin/env python3
"""大 payload + 复杂 SQL 的延迟分位（p50/p99/p999）。

做法：先 `trace sync` 建物化库，再直接用 sqlite3 往 `trit_trace` 插 `--rows` 行、
每行 `--payload` 字节的 payload，然后起服务测 5 类查询。

用法：
    python scripts/perf/deep.py --rows 2000 --payload 10000 --rounds 500
"""

from __future__ import annotations

import argparse
import pathlib
import sqlite3
import sys
import tempfile
import time

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import pg_client as pg


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    pg.add_common_args(parser)
    parser.add_argument("--rows", type=int, default=2000)
    parser.add_argument("--payload", type=int, default=10_000, help="每行 payload 字节数")
    parser.add_argument("--rounds", type=int, default=500)
    args = parser.parse_args()

    wal = pathlib.Path(tempfile.gettempdir()) / f"perf-deep-{time.time_ns()}.wal"
    db = wal.with_suffix(".sqlite")
    pg.prepare_wal(args.bin, wal, events=10, sync=True)

    conn = sqlite3.connect(db)
    blob = "x" * args.payload
    conn.executemany(
        "INSERT OR REPLACE INTO trit_trace VALUES (?,?,?,?,?,?,?,?,?,?)",
        [
            ("t", 10_000 + i, 10_000 + i, "task_selected", "Host", 1, 1, None, "h", blob)
            for i in range(args.rows)
        ],
    )
    conn.commit()
    conn.close()
    print(
        f"物化库 {db.stat().st_size / 1024 / 1024:.1f} MB"
        f"（{args.rows} 行 × {args.payload} 字节 payload）"
    )

    serve = pg.start_server(args.bin, wal, f"127.0.0.1:{args.port}")
    cases = [
        ("count(*)", "SELECT count(*) AS n FROM trit_trace"),
        (
            "WHERE actor + GROUP BY + ORDER BY",
            "SELECT kind, count(*) AS n FROM trit_trace WHERE actor = 'Host' GROUP BY kind ORDER BY kind",
        ),
        (
            "WHERE chain_seq > 500 LIMIT 100",
            "SELECT * FROM trit_trace WHERE chain_seq > 500 LIMIT 100",
        ),
        (
            "GROUP BY trace_id + max(chain_seq)",
            "SELECT trace_id, max(chain_seq) AS m FROM trit_trace GROUP BY trace_id",
        ),
        ("单行大 payload", f"SELECT payload FROM trit_trace WHERE chain_seq = {10_000 + args.rows // 2}"),
    ]
    try:
        sock = pg.connect(args.port)
        print(f"查询分位（每项 {args.rounds} 轮）：")
        for label, sql in cases:
            lat = []
            rows = 0
            for _ in range(args.rounds):
                elapsed, count = pg.one_query(sock, sql)
                lat.append(elapsed)
                rows = count
            print(
                f"  {label:<40} rows={rows:>4} "
                f"p50={pg.percentile(lat, 0.50) * 1000:>7.3f}ms "
                f"p99={pg.percentile(lat, 0.99) * 1000:>7.3f}ms "
                f"p999={pg.percentile(lat, 0.999) * 1000:>7.3f}ms "
                f"max={max(lat) * 1000:>7.2f}ms"
            )
        sock.close()
    finally:
        pg.stop_server(serve)
        pg.cleanup(wal)
    return 0


if __name__ == "__main__":
    sys.exit(main())
