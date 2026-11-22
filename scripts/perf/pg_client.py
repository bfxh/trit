"""性能/运行时验证共享件：最小 pg 协议客户端、RSS 采样、分位计算。

本模块只依赖 python 标准库；不联网、不改仓库文件。
"""

from __future__ import annotations

import json
import os
import pathlib
import socket
import struct
import subprocess
import sys

REPO = pathlib.Path(__file__).resolve().parents[2]


def default_bin() -> str:
    """优先 release，其次 debug；都要求编译过 `tritdb-pgwire` feature。"""
    for profile in ("release", "debug"):
        candidate = REPO / "target" / profile / ("trit-orch.exe" if os.name == "nt" else "trit-orch")
        if candidate.exists():
            return str(candidate)
    raise SystemExit("找不到 trit-orch 二进制；先 cargo build --release --features tritdb-pgwire")


def run_cli(bin_path: str, payload: dict, backend: str = "tritdb") -> tuple[int, str, str]:
    proc = subprocess.run(
        [bin_path, "trace", "--trace-backend", backend],
        input=json.dumps(payload).encode(),
        capture_output=True,
    )
    return proc.returncode, proc.stdout.decode().strip(), proc.stderr.decode().strip()


def startup(user: str = "u") -> bytes:
    params = b"user\0" + user.encode() + b"\0\0"
    body = struct.pack("!I", 196608) + params
    return struct.pack("!I", len(body) + 4) + body


def query(sql: str) -> bytes:
    payload = sql.encode() + b"\0"
    return b"Q" + struct.pack("!I", len(payload) + 4) + payload


def read_msg(sock) -> tuple[bytes | None, bytes | None]:
    head = b""
    while len(head) < 5:
        chunk = sock.recv(5 - len(head))
        if not chunk:
            return None, None
        head += chunk
    kind = head[:1]
    length = struct.unpack("!I", head[1:5])[0]
    body = b""
    while len(body) < length - 4:
        chunk = sock.recv(length - 4 - len(body))
        if not chunk:
            break
        body += chunk
    return kind, body


def connect(port: int, host: str = "127.0.0.1", timeout: float = 15.0):
    sock = socket.create_connection((host, port), timeout=timeout)
    sock.sendall(startup())
    while True:
        kind, body = read_msg(sock)
        if kind is None:
            raise RuntimeError("握手阶段连接被关闭")
        if kind == b"Z":
            return sock
        if kind == b"R":
            code = struct.unpack("!I", body[:4])[0]
            if code != 0:
                raise RuntimeError(f"认证要求 code={code}（本脚本只支持 trust/无认证）")


def one_query(sock, sql: str) -> tuple[float, int]:
    """返回 (耗时秒, 行数)。"""
    import time

    started = time.perf_counter()
    sock.sendall(query(sql))
    rows = 0
    while True:
        kind, body = read_msg(sock)
        if kind is None:
            raise RuntimeError("查询阶段连接被关闭")
        if kind == b"E":
            raise RuntimeError(body.decode("utf-8", "replace")[:160])
        if kind == b"D":
            rows += 1
        if kind == b"Z":
            return time.perf_counter() - started, rows


def rss_kb(pid: int) -> int | None:
    if os.name != "nt":
        try:
            text = pathlib.Path(f"/proc/{pid}/status").read_text(encoding="utf-8")
        except OSError:
            return None
        for line in text.splitlines():
            if line.startswith("VmRSS:"):
                return int(line.split()[1])
        return None
    out = subprocess.run(
        ["tasklist", "/FI", f"PID eq {pid}", "/FO", "CSV", "/NH"],
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    ).stdout.strip()
    parts = [part.strip('"') for part in out.split('","')]
    if len(parts) >= 5:
        value = parts[4].replace(",", "").replace("K", "").replace("KB", "").strip()
        try:
            return int(value)
        except ValueError:
            return None
    return None


def percentile(values: list[float], fraction: float) -> float:
    ordered = sorted(values)
    if not ordered:
        return 0.0
    return ordered[min(len(ordered) - 1, int(len(ordered) * fraction))]


def event(seq: int, trace: str = "t") -> dict:
    return {
        "schema": 1,
        "id": f"{trace}:{seq}",
        "trace_id": trace,
        "span_id": f"span-{seq}",
        "parent": None,
        "seq": seq,
        "kind": "task_selected",
        "payload": None,
        "causation_id": None,
        "correlation_id": "c",
        "logical": {"revision": 0, "epoch": 0, "fence": 0, "round": 0, "attempt": 0},
        "event_time": None,
        "observed_at": 100 + seq,
        "recorded_at": 100 + seq,
        "actor": "Host",
        "payload_hash": "",
        "prev_hash": "",
        "chain_seq": 0,
    }


def start_server(bin_path: str, wal: pathlib.Path, listen: str, env_extra: dict | None = None):
    env = dict(os.environ)
    if env_extra:
        env.update(env_extra)
    proc = subprocess.Popen(
        [bin_path, "trace", "--trace-backend", "tritdb"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=env,
    )
    proc.stdin.write(
        json.dumps({"action": "serve", "path": str(wal), "listen": listen}).encode()
    )
    proc.stdin.close()
    import time

    time.sleep(2)
    if proc.poll() is not None:
        out, err = proc.communicate()
        raise SystemExit(
            "服务启动失败："
            + (out.decode(errors="replace") + err.decode(errors="replace"))[:400]
        )
    return proc


def stop_server(proc) -> None:
    proc.terminate()
    try:
        proc.wait(timeout=10)
    except subprocess.TimeoutExpired:
        proc.kill()


def prepare_wal(bin_path: str, wal: pathlib.Path, events: int = 10, sync: bool = True) -> None:
    for suffix in ["", ".lock", ".timeidx", ".sqlite", ".sqlite-wal", ".sqlite-shm"]:
        p = pathlib.Path(str(wal) + suffix)
        if p.exists():
            p.unlink()
    batch = 500
    for start in range(1, events + 1, batch):
        chunk = [event(i) for i in range(start, min(start + batch, events + 1))]
        code, out, err = run_cli(bin_path, {"action": "append", "path": str(wal), "events": chunk})
        if code != 0:
            raise SystemExit(f"append 失败: {out or err}")
    if sync:
        code, out, err = run_cli(bin_path, {"action": "sync", "path": str(wal)})
        if code != 0:
            raise SystemExit(f"sync 失败: {out or err}")


def cleanup(wal: pathlib.Path) -> None:
    for suffix in ["", ".lock", ".timeidx", ".sqlite", ".sqlite-wal", ".sqlite-shm"]:
        p = pathlib.Path(str(wal) + suffix)
        if p.exists():
            try:
                p.unlink()
            except OSError:
                pass


def add_common_args(parser) -> None:
    parser.add_argument("--bin", default=default_bin(), help="trit-orch 二进制（需 tritdb-pgwire feature）")
    parser.add_argument("--port", type=int, default=15500)
