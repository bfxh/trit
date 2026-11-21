#!/usr/bin/env python3
"""fetch_guard：urllib 出网的逐跳地址校验（SSRF 收口，stdlib only）。

来源：Mimosa sealed 深度扫描（scan-2026-11-21T01-15-27.318Z-8e8f3901e31d，
seal digest sha256:fa6418c0d5adcce423406d87b9c94e88e555a9eb535ca6295cbf45f5ea3fbf21）
把 scripts/fetch_objaverse.py 与 scripts/fetch_textures.py 的 urlopen 判为 SSRF。
BASE/API 常量本身是公网白名单形态，首跳校验天然放行——真正的洞是 urllib
**默认跟随重定向**：任何一跳 30x 的 Location 都不再经过任何校验，就能把请求
带到 127.0.0.1、169.254.169.254（云元数据）或任意内网地址。所以校验必须逐跳：

  · 首跳：safe_urlopen 进门时校验一次；
  · 每一跳：SafeRedirectHandler.redirect_request 逐跳复检，校验通过后仍返回
    super() 构造的新 Request 以放行（返回 None 是"不跟随"，不是 fail-closed）；
  · 每一跳判三件事：scheme 仅 http/https（拒 file/ftp/data）；hostname 形态
    （localhost / *.localhost / *.local / *.localdomain / *.internal）不解析
    直接拒；host 解析为 IP 后用 ipaddress 逐项判 loopback / private /
    link-local / reserved / multicast / unspecified（IPv4-mapped / 6to4 /
    Teredo 里的 IPv4 一并剥开判）。

任一不合规 fail-closed 抛 UnsafeURLError，报文说清是哪一跳的什么形态。

用法：把 `urllib.request.urlopen(req, timeout=t)` 原地换成
`safe_urlopen(req, timeout=t)`——超时、请求头、下载语义全部不变。

自检（无网络，纯本地断言，重定向走本地 mock 传送层不出网）：
  python scripts/fetch_guard.py --selftest

边界（明确说出，不假装已解决）：DNS rebinding 不在射程内——这里校验的是
"解析后的 IP"，而连接时 urllib 还会再做一次独立解析，两次结果可以不同；
要封死需要自定义 HTTPConnection 把已验证的 IP 钉进连接，另立任务。
"""
from __future__ import annotations

import argparse
import io
import ipaddress
import socket
import threading
import urllib.error
import urllib.parse
import urllib.request
import urllib.response

ALLOWED_SCHEMES = ("http", "https")
# hostname 形态黑名单：不解析直接拒。.local 是 mDNS 面，解析结果不可信。
DENIED_HOSTNAME_EXACT = ("localhost",)
DENIED_HOSTNAME_SUFFIXES = (".localhost", ".local", ".localdomain", ".internal")

# ipaddress 逐项判：命中任何一项都拒。顺序即报文里的归因顺序。
DENIED_IP_FLAGS = (
    ("is_loopback", "loopback"),
    ("is_private", "private"),
    ("is_link_local", "link-local"),
    ("is_reserved", "reserved"),
    ("is_multicast", "multicast"),
    ("is_unspecified", "unspecified"),
)


class UnsafeURLError(urllib.error.URLError):
    """某一跳不合规（fail-closed）。reason 里说清是哪一跳、什么形态。"""


# 每线程的跳数：首跳为 0，redirect_request 每进一次 +1，safe_urlopen 进出时保存/复位。
# fetch_textures 用 ThreadPoolExecutor 并发下载，跳数必须跟线程走。
_hop_state = threading.local()


def _hop_label(hop: int) -> str:
    return "首跳" if hop == 0 else f"重定向第 {hop} 跳"


def _check_ip(addr, url: str, label: str) -> None:
    # IPv6 壳里可能藏着 IPv4 内网地址：IPv4-mapped（::ffff:x）、6to4、Teredo 的
    # client 端一并剥开，与外壳逐个过同一组判定。
    candidates = [addr]
    if addr.version == 6:
        mapped = addr.ipv4_mapped
        if mapped is not None:
            candidates.append(mapped)
        # Python 3.14 把 sixto4 改名 sixtofour，两个名字都认
        for attr in ("sixto4", "sixtofour"):
            s4 = getattr(addr, attr, None)
            if s4 is not None:
                candidates.append(s4)
                break
        teredo = addr.teredo
        if teredo is not None:
            candidates.append(teredo[1])
    for a in candidates:
        flags = [name for attr, name in DENIED_IP_FLAGS if getattr(a, attr)]
        if flags:
            raise UnsafeURLError(
                f"fetch_guard 拒绝{label}：地址 {a} 命中 {'/'.join(flags)}（url={url}）"
            )


def _resolve(host: str, url: str, label: str) -> list:
    """DNS 解析（只对非字面量 hostname 走到这）。解析失败/解不出地址一律 fail-closed。"""
    try:
        infos = socket.getaddrinfo(host, None)
    except (socket.gaierror, OSError, UnicodeError) as e:
        raise UnsafeURLError(
            f"fetch_guard 拒绝{label}：host {host!r} 解析失败，fail-closed"
            f"（{type(e).__name__}: {e}；url={url}）"
        ) from e
    out: list = []
    seen: set = set()
    for info in infos:
        sockaddr = info[4] if len(info) > 4 else None
        raw = sockaddr[0] if sockaddr else None
        if not raw or raw in seen:
            continue
        seen.add(raw)
        try:
            out.append(ipaddress.ip_address(raw))
        except ValueError as e:
            raise UnsafeURLError(
                f"fetch_guard 拒绝{label}：host {host!r} 解析出不可识别的地址形态"
                f" {raw!r}，fail-closed（url={url}）"
            ) from e
    if not out:
        raise UnsafeURLError(
            f"fetch_guard 拒绝{label}：host {host!r} 没有解析出任何地址，fail-closed（url={url}）"
        )
    return out


def _validate_hop(url: str, hop: int) -> None:
    """校验一跳：scheme 白名单 → hostname 形态 → 解析后的 IP 段。不合规即抛。"""
    label = _hop_label(hop)
    try:
        parsed = urllib.parse.urlsplit(url)
    except ValueError as e:
        raise UnsafeURLError(f"fetch_guard 拒绝{label}：URL 解析失败，fail-closed（url={url}）") from e
    scheme = (parsed.scheme or "").lower()
    if scheme not in ALLOWED_SCHEMES:
        raise UnsafeURLError(
            f"fetch_guard 拒绝{label}：scheme {scheme!r} 不在 {ALLOWED_SCHEMES} 白名单（url={url}）"
        )
    try:
        host = parsed.hostname
    except ValueError as e:
        raise UnsafeURLError(
            f"fetch_guard 拒绝{label}：host 形态不合法，fail-closed（url={url}）"
        ) from e
    if not host:
        raise UnsafeURLError(f"fetch_guard 拒绝{label}：URL 没有 host（url={url}）")
    host = host.rstrip(".").lower()
    if host in DENIED_HOSTNAME_EXACT or host.endswith(DENIED_HOSTNAME_SUFFIXES):
        raise UnsafeURLError(
            f"fetch_guard 拒绝{label}：hostname {host!r} 是本机/内网保留形态（url={url}）"
        )
    try:
        literal = ipaddress.ip_address(host)
    except ValueError:
        ips = _resolve(host, url, label)
    else:
        ips = [literal]
    for addr in ips:
        _check_ip(addr, url, label)


class SafeRedirectHandler(urllib.request.HTTPRedirectHandler):
    """redirect_request 是重定向每一跳的唯一收口点：先复检，再放行。

    放行 = 返回 super().redirect_request(...) 构造的新 Request——返回 None 会被
    urllib 当作"不跟随"，那不是 fail-closed 而是静默退化。
    """

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        _hop_state.depth = getattr(_hop_state, "depth", 0) + 1
        try:
            _validate_hop(newurl, _hop_state.depth)
        except UnsafeURLError:
            if fp is not None:
                fp.close()
            raise
        return super().redirect_request(req, fp, code, msg, headers, newurl)


_opener_lock = threading.Lock()
_opener_singleton: urllib.request.OpenerDirector | None = None


def _default_opener_factory() -> urllib.request.OpenerDirector:
    global _opener_singleton
    with _opener_lock:
        if _opener_singleton is None:
            _opener_singleton = urllib.request.build_opener(SafeRedirectHandler)
        return _opener_singleton


# 测试钩子：--selftest 把它换成不出网的本地 mock 传送层。
opener_factory = _default_opener_factory


def safe_urlopen(req_or_url, timeout=socket._GLOBAL_DEFAULT_TIMEOUT, data=None):
    """urlopen 的逐跳校验替身：Request 或 URL 均可，超时/请求头行为与 urlopen 一致。

    首跳在进门时校验；后续每一跳由 SafeRedirectHandler 在 redirect_request 复检。
    """
    if isinstance(req_or_url, urllib.request.Request):
        req = req_or_url
    else:
        req = urllib.request.Request(req_or_url, data=data)
    prev = getattr(_hop_state, "depth", None)
    _hop_state.depth = 0
    try:
        _validate_hop(req.full_url, 0)
        return opener_factory().open(req, timeout=timeout)
    finally:
        _hop_state.depth = prev


# ============================================================
# --selftest：无网络，纯本地断言。字面量 IP 不走 DNS，hostname 放行形态
# 注入假 resolver，重定向链走本地 mock 传送层，全程不出网。
# ============================================================

def _set_global(name: str, value):
    """selftest 的补丁钩子：换模块级注入点，返回旧值供 finally 复原。"""
    old = globals()[name]
    globals()[name] = value
    return old


def _selftest() -> int:
    passed = 0
    failed = 0

    def check(cond: bool, name: str, detail: str = "") -> None:
        nonlocal passed, failed
        if cond:
            passed += 1
            print(f"  PASS  {name}")
        else:
            failed += 1
            print(f"  FAIL  {name}  ({detail})")

    def allows(url: str, name: str) -> None:
        try:
            _validate_hop(url, 0)
            check(True, name)
        except UnsafeURLError as e:
            check(False, name, str(e))

    def rejects(url: str, name: str, needle: str) -> None:
        try:
            _validate_hop(url, 0)
            check(False, name, "没有抛 UnsafeURLError")
        except UnsafeURLError as e:
            check(needle in str(e), name, str(e))

    print("fetch_guard selftest：无网络，纯本地断言（重定向走本地 mock，不出网）")

    # --- 放行形态（字面量公网 IP，不经 DNS） ---
    allows("https://8.8.8.8/datasets/allenai/objaverse/resolve/main/x.json.gz",
           "T1 https 公网字面 IP 放行")
    allows("http://93.184.216.34/", "T2 http 公网字面 IP 放行")

    # --- scheme 白名单 ---
    rejects("file:///etc/passwd", "T3 file:// 拒", "scheme")
    rejects("ftp://8.8.8.8/x", "T4 ftp:// 拒", "scheme")
    rejects("data:text/plain,hi", "T5 data: 拒", "scheme")

    # --- hostname 形态（不解析直接拒） ---
    rejects("http://localhost/x", "T6 localhost 形态拒", "localhost")
    rejects("http://foo.local/x", "T7 *.local 形态拒", "hostname")

    # --- 解析后 IP 段（字面量，逐项归因） ---
    rejects("http://127.0.0.1:8080/x", "T8 127.0.0.1（loopback）拒", "loopback")
    rejects("http://169.254.169.254/latest/meta-data/", "T9 169.254 云元数据（link-local）拒", "link-local")
    rejects("http://10.1.2.3/x", "T10 10.x（private）拒", "private")
    rejects("http://192.168.1.5/x", "T11 192.168.x（private）拒", "private")
    rejects("http://224.0.0.1/x", "T12 多播（multicast）拒", "multicast")
    rejects("http://[::1]/x", "T13 [::1]（IPv6 loopback）拒", "loopback")
    rejects("http://[fe80::1]/x", "T14 fe80 链路本地 IPv6 拒", "link-local")
    rejects("http://[::ffff:127.0.0.1]/x", "T15 IPv4-mapped 壳里的 loopback 拒", "loopback")
    allows("http://[2606:4700::1111]/x", "T16 公网 IPv6 字面量放行")

    # --- hostname 放行形态：BASE/TREE 就是这个形状，注入假 resolver，不出网 ---
    old_resolve = _set_global("_resolve", lambda host, url, label: [ipaddress.ip_address("8.8.8.8")])
    try:
        allows("https://huggingface.co/datasets/allenai/objaverse/resolve/main/metadata/000-000.json.gz",
               "T17 公网 hostname（BASE/TREE 形态）放行")
        allows("https://ambientcg.com/api/v2/full_json?type=Material&limit=100",
               "T18 公网 hostname（API 形态）放行")
    finally:
        _set_global("_resolve", old_resolve)

    # --- 本地 mock 传送层：重定向到私网必须拒，且报文点名是哪一跳 ---
    from email.message import Message

    class _MockHTTP(urllib.request.HTTPHandler):
        """不出网的传送层：按脚本逐次给响应（302 Location / 200 收尾）。"""

        def __init__(self, script):
            super().__init__()
            self._script = list(script)

        def http_open(self, req):
            code, location = self._script.pop(0)
            headers = Message()
            if location:
                headers["Location"] = location
            resp = urllib.response.addinfourl(io.BytesIO(b"mock"), headers, req.full_url, code)
            resp.code = code
            resp.msg = "Mock"
            return resp

    def _mock_opener(script):
        return urllib.request.build_opener(SafeRedirectHandler(), _MockHTTP(script))

    real_factory = opener_factory
    # T19：重定向跳进私网 → 拒，报文说清是重定向第 1 跳、地址、命中段
    _set_global("opener_factory", lambda: _mock_opener([(302, "http://10.9.8.7/x")]))
    try:
        try:
            safe_urlopen("http://8.8.8.8/start", timeout=1)
            check(False, "T19 重定向到私网被拒", "没有抛 UnsafeURLError")
        except UnsafeURLError as e:
            text = str(e)
            check(
                "重定向第 1 跳" in text and "10.9.8.7" in text and "private" in text,
                "T19 重定向到私网被拒（报文点名跳数/地址/段）",
                text,
            )
    finally:
        _set_global("opener_factory", real_factory)

    # T20：重定向跳到公网 → 放行，重定向链真的走完（校验后仍返回 Request 以放行）
    _set_global("opener_factory",
                lambda: _mock_opener([(302, "http://8.8.8.8/ok"), (200, None)]))
    try:
        with safe_urlopen("http://8.8.8.8/start", timeout=1) as r:
            body_ok = r.read() == b"mock"
        check(r.geturl() == "http://8.8.8.8/ok" and r.code == 200 and body_ok,
              "T20 重定向到公网放行（跳数走完、最终 URL 正确）",
              f"url={r.geturl()} code={r.code}")
    finally:
        _set_global("opener_factory", real_factory)

    # T21：safe_urlopen 首跳即拒（不触传送层）
    try:
        safe_urlopen("file:///etc/not-real", timeout=1)
        check(False, "T21 safe_urlopen 首跳拒 file://", "没有抛 UnsafeURLError")
    except UnsafeURLError as e:
        check("首跳" in str(e) and "scheme" in str(e), "T21 safe_urlopen 首跳拒 file://", str(e))

    print(f"SELFTEST {passed} passed, {failed} failed（共 {passed + failed} 条）")
    return 0 if failed == 0 else 1


def main() -> int:
    ap = argparse.ArgumentParser(
        prog="fetch_guard",
        description="urllib 出网逐跳地址校验（SSRF 收口）",
        epilog="自检：python scripts/fetch_guard.py --selftest",
    )
    ap.add_argument("--selftest", action="store_true", help="无网络本地自检（纯本地断言，不出网）")
    args = ap.parse_args()
    if args.selftest:
        return _selftest()
    ap.print_help()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
