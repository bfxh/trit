#!/usr/bin/env python3
"""长稳：指定秒数的并发压测 + RSS 采样，看内存是否缓慢增长。

用法：
    python scripts/perf/soak.py --duration 3600 --clients 8 --interval 30
"""

from __future__ import annotations

import argparse
import pathlib
import sys
import tempfile
import threading
import time

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import pg_client as pg


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    pg.add_common_args(parser)
    parser.add_argument("--duration", type=int, default=120, help="秒")
    parser.add_argument("--clients", type=int, default=8)
    parser.add_argument("--interval", type=float, default=30, help="RSS 采样间隔（秒）")
    args = parser.parse_args()

    wal = pathlib.Path(tempfile.gettempdir()) / f"perf-soak-{time.time_ns()}.wal"
    pg.prepare_wal(args.bin, wal, events=10, sync=True)
    serve = pg.start_server(args.bin, wal, f"127.0.0.1:{args.port}")

    samples: list[int] = []
    stop = threading.Event()

    def sampler():
        while not stop.is_set():
            value = pg.rss_kb(serve.pid)
            if value:
                samples.append(value)
            time.sleep(args.interval)

    threading.Thread(target=sampler, daemon=True).start()

    stats = {"ok": 0, "fail": 0}
    lat: list[float] = []
    lock = threading.Lock()
    deadline = time.time() + args.duration

    def worker():
        ok = 0
        fail = 0
        local: list[float] = []
        try:
            sock = pg.connect(args.port)
            while time.time() < deadline:
                try:
                    elapsed, _ = pg.one_query(sock, "SELECT count(*) AS n FROM trit_trace")
                    ok += 1
                    local.append(elapsed)
                except Exception:
                    fail += 1
                    break
            sock.close()
        except Exception:
            fail += 1
        with lock:
            stats["ok"] += ok
            stats["fail"] += fail
            lat.extend(local)

    threads = [threading.Thread(target=worker) for _ in range(args.clients)]
    started = time.time()
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()
    elapsed = time.time() - started
    stop.set()
    pg.stop_server(serve)
    pg.cleanup(wal)

    total = stats["ok"] + stats["fail"]
    print(
        f"soak {args.clients} 并发 {elapsed:.0f}s: 请求 {total} ok={stats['ok']} fail={stats['fail']} "
        f"qps={stats['ok'] / elapsed if elapsed else 0:.0f}"
    )
    if lat:
        print(
            f"  p50={pg.percentile(lat, 0.50) * 1000:.3f}ms "
            f"p99={pg.percentile(lat, 0.99) * 1000:.3f}ms "
            f"p999={pg.percentile(lat, 0.999) * 1000:.3f}ms"
        )
    if samples:
        print(
            f"  RSS 采样 {len(samples)} 次: {samples[0]} KB -> {samples[-1]} KB, "
            f"峰值 {max(samples)} KB"
        )
        print(f"  RSS 序列: {samples}")
    return 0 if stats["fail"] == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
