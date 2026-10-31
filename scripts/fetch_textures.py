"""下载 ambientCG 材质子集（CC0，1K-JPG），只留 Color 贴图。

用法：python scripts/fetch_textures.py [OUT_DIR] [COUNT] [CATEGORY] [JOBS]
  OUT_DIR  默认 D:/KF/trit-data/textures/ambientcg
  COUNT    默认 8
  CATEGORY 可选（fabric / wood / metal / stone / tiles / concrete …）
  JOBS     并发数，默认 6

为什么用 Python：curl 在 Git Bash 里对 Windows 路径/CRLF 组合会静默产出 0 字节
（2026-10-31 实测 0/8 全失败），urllib + zipfile 一次到位；这只是**数据准备**，不参与训练。
"""
from __future__ import annotations

import json
import shutil
import sys
import time
import urllib.request
import zipfile
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

UA = {"User-Agent": "Mozilla/5.0 (trit-parts dataset fetch)"}
API = "https://ambientcg.com/api/v2/full_json"


def get_json(url: str) -> dict:
    req = urllib.request.Request(url, headers=UA)
    with urllib.request.urlopen(req, timeout=180) as r:
        return json.load(r)


def list_assets(count: int, cat: str, page: int = 100, cache: Path | None = None) -> list:
    """分页拉清单：单次 limit=1300 的响应约 8.4 MB，服务端要 2–3 分钟（实测 >120 s 才传完），
    分页到 100 条一次才稳定。清单落盘缓存——重跑时不该再花 10 分钟拉一遍。"""
    import json as _json

    if cache is not None and cache.exists():
        got = _json.loads(cache.read_text(encoding="utf-8"))
        if len(got) >= count:
            print(f"清单缓存命中 {len(got)} 个", flush=True)
            return got[:count]
    out: list = []
    offset = 0
    while len(out) < count:
        n = min(page, count - len(out))
        url = f"{API}?type=Material&limit={n}&offset={offset}&include=downloadData"
        if cat:
            url += f"&category={cat}"
        got = get_json(url).get("foundAssets", [])
        if not got:
            break
        out.extend(got)
        offset += len(got)
        if len(got) < n:
            break
    return out


def one_k_zip(asset: dict) -> str | None:
    try:
        dl = asset["downloadFolders"]["default"]["downloadFiletypeCategories"]["zip"]["downloads"]
    except KeyError:
        return None
    for x in dl:
        if x.get("attribute", "").startswith("1K"):
            return x["fullDownloadPath"]
    return dl[0]["fullDownloadPath"] if dl else None


def disk_free_gb(p: Path) -> float:
    return shutil.disk_usage(p).free / 1e9


def download(url: str, dst: Path, retries: int = 2) -> bool:
    for i in range(retries):
        try:
            req = urllib.request.Request(url, headers=UA)
            with urllib.request.urlopen(req, timeout=60) as r:
                data = r.read()
            if len(data) < 100_000:
                raise RuntimeError(f"体积异常 {len(data)}")
            dst.write_bytes(data)
            return True
        except Exception as e:
            print(f"  retry {i + 1}/{retries}: {e}", flush=True)
            time.sleep(0.5 + i)
    return False


def main() -> int:
    out = Path(sys.argv[1] if len(sys.argv) > 1 else "D:/KF/trit-data/textures/ambientcg")
    count = int(sys.argv[2]) if len(sys.argv) > 2 else 8
    cat = sys.argv[3] if len(sys.argv) > 3 else ""
    jobs = int(sys.argv[4]) if len(sys.argv) > 4 else 6
    out.mkdir(parents=True, exist_ok=True)

    free = disk_free_gb(out)
    if free < 5:
        print(f"DISK_LOW {free:.1f} GB < 5 GB，停止")
        return 2
    print(
        f"下载到 {out}（剩余 {free:.1f} GB），{count} 个材质"
        + (f"，类别={cat}" if cat else "")
        + f"，并发 {jobs}",
        flush=True,
    )

    assets = list_assets(count, cat, cache=out / f"api_{count}_{cat or 'all'}.json")
    (out / f"api_{count}_{cat or 'all'}.json").write_text(
        __import__("json").dumps(assets), encoding="utf-8"
    )
    print(f"清单 {len(assets)} 个", flush=True)

    def one(a: dict) -> bool:
        aid = a["assetId"]
        dst = out / aid
        if any(dst.glob("*Color*")):
            return True
        zurl = one_k_zip(a)
        if not zurl:
            print(f"{aid} 无 zip 下载项", flush=True)
            return False
        tmp = out / f"{aid}.zip"
        if not download(zurl, tmp):
            print(f"{aid} FAIL", flush=True)
            return False
        try:
            with zipfile.ZipFile(tmp) as z:
                members = z.namelist()
                keep = [m for m in members if "color" in m.lower() and not m.endswith("/")]
                if not keep:
                    print(f"{aid} zip 里没有 Color（{len(members)} 项）", flush=True)
                    return False
                dst.mkdir(parents=True, exist_ok=True)
                for m in keep:
                    with z.open(m) as f, open(dst / Path(m).name, "wb") as g:
                        shutil.copyfileobj(f, g)
            print(f"{aid} ok", flush=True)
            return True
        finally:
            tmp.unlink(missing_ok=True)

    import time as _t

    t0 = _t.time()
    with ThreadPoolExecutor(max_workers=max(1, jobs)) as ex:
        results = list(ex.map(one, assets))
    ok = sum(results)
    dt = _t.time() - t0
    print(f"FETCH_DONE {ok}/{len(assets)}（{dt / 60:.1f} 分钟，{60 * len(assets) / max(dt, 1e-9):.1f} 个/分钟）")
    return 0 if ok == count else 1


if __name__ == "__main__":
    sys.exit(main())
