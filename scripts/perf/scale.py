#!/usr/bin/env python3
"""并发阶梯：单进程多线程客户端，测 QPS、延迟分位与服务 RSS。

用法：
    python scripts/perf/scale.py --clients 8,32,64 --seconds 30
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


def run_round(bin_path: str, wal: pathlib.Path, clients: int, seconds: int, port: int) -> dict:
    serve = pg.start_server(bin_path, wal, f"127.0.0.1:{port}")
    samples: list[int] = []
    stop = threading.Event()

    def sampler():
        while not stop.is_set():
            value = pg.rss_kb(serve.pid)
            if value:
                samples.append(value)
            time.sleep(1)

    threading.Thread(target=sampler, daemon=True).start()

    stats = {"ok": 0, "fail": 0}
    lat: list[float] = []
    lock = threading.Lock()
    deadline = time.time() + seconds

    def worker():
        ok = 0
        fail = 0
        local: list[float] = []
        try:
            sock = pg.connect(port)
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

    threads = [threading.Thread(target=worker) for _ in range(clients)]
    started = time.time()
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()
    elapsed = time.time() - started
    stop.set()
    pg.stop_server(serve)

    return {
        "clients": clients,
        "ok": stats["ok"],
        "fail": stats["fail"],
        "qps": stats["ok"] / elapsed if elapsed else 0,
        "p50": pg.percentile(lat, 0.50) * 1000,
        "p99": pg.percentile(lat, 0.99) * 1000,
        "p999": pg.percentile(lat, 0.999) * 1000,
        "rss_start": samples[0] if samples else 0,
        "rss_end": samples[-1] if samples else 0,
        "rss_peak": max(samples) if samples else 0,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    pg.add_common_args(parser)
    parser.add_argument("--clients", default="8,32,64", help="并发数，逗号分隔")
    parser.add_argument("--seconds", type=int, default=30)
    parser.add_argument("--events", type=int, default=10)
    args = parser.parse_args()

    wal = pathlib.Path(tempfile.gettempdir()) / f"perf-scale-{time.time_ns()}.wal"
    pg.prepare_wal(args.bin, wal, events=args.events, sync=True)
    failures = 0
    port = args.port
    for clients in [int(value) for value in args.clients.split(",") if value]:
        try:
            result = run_round(args.bin, wal, clients, args.seconds, port)
        except SystemExit as error:
            print(f"并发 {clients}: {error}")
            failures += 1
            port += 1
            continue
        port += 1
        print(
            f"并发 {clients:>3} x {args.seconds}s: ok={result['ok']} fail={result['fail']} "
            f"qps={result['qps']:.0f} p50={result['p50']:.3f}ms p99={result['p99']:.3f}ms "
            f"p999={result['p999']:.3f}ms RSS {result['rss_start']}->{result['rss_end']}KB "
            f"peak={result['rss_peak']}KB"
        )
        if result["fail"]:
            failures += 1
    pg.cleanup(wal)
    return 0 if failures == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
