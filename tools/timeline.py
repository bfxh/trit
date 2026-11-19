#!/usr/bin/env python3
"""timeline: problems as time series, not as permanent states.

A fixed status ("open") makes every problem look equally permanent; a closed status
("won't do") makes it permanent in the other direction. Neither captures the truth that
matters: this does not work *yet*, and there is a date at which the reason may no longer
hold. Boundaries are not laws -- they are claims with an expiry.

So every item carries first_seen, last_seen, every occurrence with its evidence, and a
review_at. "accepted" is not "forever" either: an accepted limitation still comes due.
`due` is what turns "temporary" from a phrase into a queue.

This module is the single implementation. Entry points (timeline.sh, timeline.ps1) only
forward argv, so the two shells cannot drift apart.
"""

import json
import os
import re
import sys
import uuid
from datetime import datetime, timedelta

MODES = {"open", "touch", "resolve", "accept", "obsolete", "due", "trend", "list"}
KINDS = {"blocker", "boundary", "limitation", "risk"}


# --- argument parsing -------------------------------------------------------------
# Accepts both "-Mode open" and "--mode open", so muscle memory from either shell works.
def parse_args(argv):
    # bare_mode: the first non-flag token is the mode, so `timeline.sh due -Root x` works
    # without writing `-Mode`. Both forms are accepted because both are natural in a shell.
    positional = [x for x in argv if not x.startswith("-")]
    out = {}
    i = 0
    while i < len(argv):
        a = argv[i]
        if not a.startswith("-"):
            i += 1
            continue
        name = a.lstrip("-")
        if name == "Mode":
            name = "mode"
        elif name == "Root":
            name = "root"
        elif name == "Kind":
            name = "kind"
        elif name == "Title":
            name = "title"
        elif name == "Detail":
            name = "detail"
        elif name == "Evidence":
            name = "evidence"
        elif name == "Id":
            name = "id"
        elif name == "ReviewAfterDays":
            name = "review_after_days"
        elif name == "Now":
            name = "now"
        else:
            name = name.lower()
        val = argv[i + 1] if i + 1 < len(argv) else ""
        out[name] = val
        i += 2
    if "mode" not in out and positional and positional[0] in MODES:
        out["mode"] = positional[0]
    return out


def fail(msg, code=2):
    print(msg)
    sys.exit(code)


# --- time ---------------------------------------------------------------------------
# Timestamps must round-trip with the PowerShell-era store, which wrote 7 fractional
# digits (DateTime.ToString('o')). Python before 3.11 rejects anything but 3 or 6, so
# normalize the fraction before parsing rather than trusting the producer.
FRACTION = re.compile(r"(\d{2}:\d{2}:\d{2})\.(\d+)")


def parse_time(s):
    if not s:
        return None
    s = str(s).strip()
    if s.endswith("Z"):
        s = s[:-1] + "+00:00"
    m = FRACTION.search(s)
    if m:
        frac = m.group(2)[:6].ljust(6, "0")
        s = s[: m.start()] + f"{m.group(1)}.{frac}" + s[m.end():]
    try:
        dt = datetime.fromisoformat(s)
    except ValueError:
        dt = None
    if dt is None:
        for fmt in ("%Y-%m-%d", "%Y/%m/%d %H:%M:%S", "%m/%d/%Y %H:%M:%S"):
            try:
                dt = datetime.strptime(s, fmt)
                break
            except ValueError:
                continue
    if dt is None:
        fail(f"timeline: 无法解析时间 {s!r}")
    # Store values carry an offset; a bare "-Now 2026-01-01T00:00:00" does not. Comparing
    # a naive against an aware datetime raises TypeError, so pin every naive value to the
    # local zone -- one machine, one zone, no surprise DST arithmetic.
    if dt.tzinfo is None:
        dt = dt.replace(tzinfo=datetime.now().astimezone().tzinfo)
    return dt


def iso(dt):
    # seconds precision, explicit local offset: readable by bash `date`, PS `[DateTime]::Parse`
    # and Python alike. Fractional seconds bought nothing but parse fragility.
    return dt.isoformat(timespec="seconds")


def short_date(s):
    t = parse_time(s)
    return t.strftime("%Y-%m-%d") if t else "        -"


def days_between(a, b):
    d = (parse_time(b) - parse_time(a)).total_seconds() / 86400.0
    return round(d, 1)


# --- store --------------------------------------------------------------------------
class Store:
    def __init__(self, root):
        self.root = root
        self.dir = os.path.join(root, ".agent")
        self.path = os.path.join(self.dir, "timeline.json")
        self.items = []
        if os.path.exists(self.path):
            with open(self.path, "r", encoding="utf-8-sig") as f:
                doc = json.load(f)
            self.items = list(doc.get("items") or [])

    def save(self):
        os.makedirs(self.dir, exist_ok=True)
        tmp = self.path + ".tmp"
        with open(tmp, "w", encoding="utf-8", newline="\n") as f:
            json.dump({"version": 1, "items": self.items}, f, ensure_ascii=False, indent=2)
            f.write("\n")
        os.replace(tmp, self.path)

    def find(self, iid):
        for e in self.items:
            if e.get("id") == iid:
                return e
        return None


def item_key(kind, title):
    import hashlib
    return hashlib.sha256(f"{kind}|{title}".encode("utf-8")).hexdigest()[:8]


def cmd_open(st, a, now):
    title = a.get("title", "").strip()
    if not title:
        fail("timeline: -Title is required")
    ev = a.get("evidence", "").strip()
    if not ev:
        fail("timeline: 拒绝 -- 开一条问题也要带 -Evidence（否则无从判断它是不是真的存在）", 1)
    kind = a.get("kind", "blocker")
    if kind not in KINDS:
        fail(f"timeline: -Kind 必须是 {sorted(KINDS)} 之一")
    key = item_key(kind, title)
    for e in st.items:
        if e.get("key") == key:
            print(f"timeline: 已存在同标题条目 {e['id']}，用 -Mode touch 追加一次发生")
            sys.exit(3)
    days = int(a.get("review_after_days") or 14)
    iid = kind[0] + uuid.uuid4().hex[:8]
    st.items.append({
        "id": iid,
        "key": key,
        "kind": kind,
        "title": title,
        "detail": a.get("detail", ""),
        "status": "open",
        "first_seen": iso(now),
        "last_seen": iso(now),
        "review_at": iso(now + timedelta(days=days)),
        "occurrences": [{"at": iso(now), "evidence": ev}],
        "resolution": None,
        "accepted": None,
    })
    st.save()
    print(f"timeline: opened {iid} [{kind}] {title}  (复核于 {short_date(iso(now + timedelta(days=days)))}）")


def need_item(st, a):
    iid = a.get("id", "").strip()
    if not iid:
        fail("timeline: -Id is required")
    e = st.find(iid)
    if not e:
        fail(f"timeline: 没有 id={iid}")
    return iid, e


def cmd_touch(st, a, now):
    iid, e = need_item(st, a)
    if e.get("status") != "open":
        fail(f"timeline: {iid} 已是 {e.get('status')}，要重开请 -Mode open", 3)
    ev = a.get("evidence", "").strip()
    if not ev:
        fail("timeline: 拒绝 -- touch 必须带 -Evidence", 1)
    days = int(a.get("review_after_days") or 14)
    e.setdefault("occurrences", []).append({"at": iso(now), "evidence": ev})
    e["last_seen"] = iso(now)
    # Recurrence means it is not a one-off, so push the next reminder out instead of
    # nagging every time it reappears.
    e["review_at"] = iso(now + timedelta(days=days))
    st.save()
    print(f"timeline: {iid} touched -- 第 {len(e['occurrences'])} 次发生，复核于 {short_date(e['review_at'])}")


def cmd_resolve(st, a, now):
    iid, e = need_item(st, a)
    ev = a.get("evidence", "").strip()
    if not ev:
        fail("timeline: 拒绝 -- 结案必须带 -Evidence（'修好了' 不能只是句话）", 1)
    e["status"] = "resolved"
    e["resolution"] = {"at": iso(now), "evidence": ev}
    e["last_seen"] = iso(now)
    st.save()
    print(f"timeline: {iid} resolved -- 存活 {days_between(e['first_seen'], iso(now))} 天，{len(e.get('occurrences') or [])} 次发生")


def cmd_accept(st, a, now):
    iid, e = need_item(st, a)
    ev = a.get("evidence", "").strip()
    if not ev:
        fail("timeline: 拒绝 -- accept 要说明为什么接受、以及什么条件下会重新审视", 1)
    days = int(a.get("review_after_days") or 14)
    until = iso(now + timedelta(days=days))
    e["status"] = "accepted"
    e["accepted"] = {"at": iso(now), "review_at": until, "why": ev}
    e["review_at"] = until
    st.save()
    print(f"timeline: {iid} accepted -- 但 {short_date(until)} 会重新到期（接受不等于永久）")


def cmd_obsolete(st, a, now):
    iid, e = need_item(st, a)
    e["status"] = "obsolete"
    e["last_seen"] = iso(now)
    st.save()
    print(f"timeline: {iid} obsolete -- 前提已经不存在了")


def is_due(e, now):
    if e.get("status") not in ("open", "accepted"):
        return False
    r = parse_time(e.get("review_at"))
    return r is not None and r <= now


def cmd_due(st, a, now):
    due = [e for e in st.items if is_due(e, now)]
    if not due:
        print(f"timeline: 没有到期项（共 {len(st.items)} 条）")
        return 0
    due.sort(key=lambda e: parse_time(e["review_at"]))
    print(f"timeline: {len(due)} 条到期，需要重新确认（不是自动关闭）")
    for d in due:
        since = days_between(d.get("first_seen"), iso(now))
        print(f"  [{d.get('kind')}/{d.get('status')}] {d.get('id')} {d.get('title')}")
        print(f"      首次 {short_date(d.get('first_seen'))}  已存活 {since}d  "
              f"发生 {len(d.get('occurrences') or [])} 次  上次 {short_date(d.get('last_seen'))}")
    print("提示：到期项要么 resolve（带证据），要么 touch（还是存在），要么 accept（本轮不做但给定新期限）")
    return 1


def cmd_trend(st, a, now):
    iid, e = need_item(st, a)
    print(f"{e['id']}  [{e.get('kind')}/{e.get('status')}] {e.get('title')}")
    print(f"首次 {e.get('first_seen')}   最近 {e.get('last_seen')}   复核 {e.get('review_at')}")
    print()
    print(f"{'时间':<12} {'事件':<10} 证据")
    series = []
    for o in e.get("occurrences") or []:
        series.append((o.get("at"), "发生", o.get("evidence")))
    if e.get("resolution"):
        series.append((e["resolution"].get("at"), "结案", e["resolution"].get("evidence")))
    if e.get("accepted"):
        series.append((e["accepted"].get("at"), "接受", e["accepted"].get("why")))
    # sort on the parsed time, not the string: offsets and precisions differ between writers
    series.sort(key=lambda s: parse_time(s[0]) or datetime.min)
    for at, what, ev in series:
        print(f"{short_date(at):<12} {what:<10} {ev}")
    return 0


def cmd_list(st, a, now):
    print(f"{'id':<10} {'kind':<9} {'status':<10} {'次数':<6} title")
    rows = sorted(st.items, key=lambda e: (e.get("last_seen") or ""), reverse=True)
    for e in rows:
        print(f"{e.get('id'):<10} {e.get('kind'):<9} {e.get('status'):<10} "
              f"{len(e.get('occurrences') or []):<6} {e.get('title')}")
    return 0


def main(argv):
    a = parse_args(argv)
    mode = a.get("mode", "")
    if mode not in MODES:
        fail(f"timeline: -Mode 必须是 {sorted(MODES)} 之一")
    root = a.get("root", "").strip()
    if not root:
        fail("timeline: -Root is required")
    if not os.path.isdir(root):
        fail(f"timeline: -Root 不存在 {root}")
    now = parse_time(a["now"]) if a.get("now") else datetime.now().astimezone()
    st = Store(os.path.abspath(root))
    fn = {
        "open": cmd_open, "touch": cmd_touch, "resolve": cmd_resolve,
        "accept": cmd_accept, "obsolete": cmd_obsolete, "due": cmd_due,
        "trend": cmd_trend, "list": cmd_list,
    }[mode]
    return fn(st, a, now) or 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
