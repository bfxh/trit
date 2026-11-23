#!/usr/bin/env python3
"""alignment-gate: 目标 -> 需求 -> 产物/证据/变更 的显式对齐门。

许多智能体只回答“做完了”，却不回答“这件事和那个目标/需求/产物是不是同一个东西”。
本门把这种关系写成可审计的 alignment/1（单目标，兼容旧清单）、alignment/2
（单根目标树 + 子目标）或 alignment/3（证据再绑定到本次实际成功的 run 命令）
清单，并在动手前拒绝：

  - 需求没有父目标，或父链断掉/成环；
  - 子目标没有父目标、父链断掉/成环，或出现多个根目标；
  - must 需求没有产物、没有证据；strict 下 should 也一样；
  - 引用了不存在的 artifact/evidence id；
  - 产物/证据文件不存在、哈希不符、locator 指不到内容；
  - alignment/3 的 must 证据没有声明会证明它的 run 命令，或该命令没有在本
    次 run 的成功记录里（post-run 复核）；
  - 变更文件没有挂到任何需求，或落进显式 non_goal 路径；
  - 实际变更集已知时，requirement 声明的 changes 没有实际兑现（空集也不是免检）；
  - 两条需求声明互相矛盾；
  - 路径绝对化、`..` 逃逸或符号链接逃出 root。

用法：
  python tools/alignment_gate.py --root . --manifest .agent/alignment.json \
      --changed src/a.rs,tests/a.rs --json
  python tools/alignment_gate.py --root . --manifest .agent/alignment.json \
      --changed-set --json          # actual changed set known, even if empty
  python tools/alignment_gate.py --root . --manifest .agent/alignment.json --lenient
  python tools/alignment_gate.py --root . --manifest .agent/alignment.json \
      --run-command "python -m pytest tests/test_a.py"
  python tools/alignment_gate.py scaffold --root . --manifest .agent/alignment.json \
      --goal "目标" --requirement "需求一" --requirement "需求二"
  python tools/alignment_gate.py self-test
  python tools/alignment_gate.py stress --cases 100

退出码：0 aligned；1 unaligned；2 坏调用；3 unknown（清单缺失/读不出）。
"""

from __future__ import annotations

import argparse
import copy
import fnmatch
import hashlib
import json
import os
import random
import re
import sys
import tempfile

SCHEMA_V1 = "alignment/1"
SCHEMA_V2 = "alignment/2"
SCHEMA_V3 = "alignment/3"
SCHEMAS = (SCHEMA_V1, SCHEMA_V2, SCHEMA_V3)
SCHEMA = SCHEMA_V1  # 旧清单的默认形状（helpers 与旧测试沿用）
TOP_FIELDS = {"schema", "goal", "goals", "requirements", "artifacts", "evidence",
              "non_goals", "allow_unlinked", "notes"}
GOAL_V1_FIELDS = {"id", "text", "notes"}
GOAL_V2_FIELDS = {"id", "text", "parent", "notes"}
GOAL_FIELDS = GOAL_V1_FIELDS  # 兼容旧名字
REQ_FIELDS = {"id", "text", "priority", "parent", "artifacts", "evidence",
              "changes", "contradicts", "notes", "tags", "owner"}
ART_FIELDS = {"id", "path", "locator", "sha256", "kind", "notes", "tags"}
EV_FIELDS = {"id", "path", "locator", "sha256", "notes", "tags"}
EV_FIELDS_V3 = EV_FIELDS | {"kind", "command"}
NG_FIELDS = {"id", "text", "paths", "artifacts", "evidence", "notes"}
PRIORITIES = ("must", "should", "may")
EVIDENCE_KINDS = ("file", "run")
LOCATOR_RX = re.compile(r"^L(\d+)(?:-L?(\d+))?$")
SHA_RX = re.compile(r"^[0-9a-fA-F]{64}$")
CHANGE_BAD_RX = re.compile(r"^[A-Za-z]:[\\/]")


class AlignmentFailure(Exception):
    def __init__(self, code, message, node=None):
        super().__init__(message)
        self.code = code
        self.message = message
        self.node = node


def _display(root, path):
    try:
        rel = os.path.relpath(os.path.abspath(path), os.path.abspath(root))
    except ValueError:
        return os.path.basename(path)
    if rel == ".":
        return "."
    if rel.startswith(os.pardir + os.sep) or rel == os.pardir:
        return os.path.basename(path)
    return rel.replace("\\", "/")


def _safe_rel(root, rel, node, what):
    """Resolve a repo-relative path and refuse escape (absolute, .., symlink)."""
    if not isinstance(rel, str) or not rel.strip():
        raise AlignmentFailure("bad-path", f"{what} 路径为空或不是字符串", node)
    raw = rel.strip().replace("\\", "/")
    if os.path.isabs(rel) or CHANGE_BAD_RX.match(rel):
        raise AlignmentFailure("bad-path", f"{what} 路径必须是仓库相对路径：{rel!r}", node)
    norm = os.path.normpath(raw).replace("\\", "/")
    if norm in ("", ".") or norm == ".." or norm.startswith("../"):
        raise AlignmentFailure("bad-path", f"{what} 路径逃出 root：{rel!r}", node)
    full = os.path.abspath(os.path.join(root, norm))
    root_real = os.path.realpath(root)
    full_real = os.path.realpath(full)
    try:
        common = os.path.commonpath([root_real, full_real])
    except ValueError:
        raise AlignmentFailure("bad-path", f"{what} 路径与 root 不同盘：{rel!r}", node)
    if os.path.normcase(common) != os.path.normcase(root_real):
        raise AlignmentFailure("bad-path", f"{what} 路径经符号链接逃出 root：{rel!r}", node)
    return norm, full


def _is_file(root, rel, node, what):
    norm, full = _safe_rel(root, rel, node, what)
    if not os.path.isfile(full):
        raise AlignmentFailure(f"{what}-missing", f"{what} 文件不存在：{norm}", node)
    return norm, full


def _line_text(full):
    try:
        with open(full, "r", encoding="utf-8", errors="replace") as fh:
            return fh.read()
    except OSError as e:
        raise AlignmentFailure("locator-unreadable", f"文件读不出：{e}")


def _check_locator(full, locator, node):
    if locator is None:
        return
    if not isinstance(locator, str) or not locator.strip():
        raise AlignmentFailure("locator-invalid", "locator 必须是非空字符串", node)
    token = locator.strip()
    m = LOCATOR_RX.match(token)
    if m:
        start = int(m.group(1))
        end = int(m.group(2) or start)
        if start < 1 or end < start:
            raise AlignmentFailure("locator-invalid", f"locator 行号非法：{token}", node)
        text = _line_text(full)
        lines = len(text.splitlines())
        if end > lines:
            raise AlignmentFailure("locator-out-of-range",
                                   f"locator {token} 超出文件行数 {lines}", node)
        return
    if token.startswith("text:"):
        needle = token[5:]
        if not needle:
            raise AlignmentFailure("locator-invalid", "text: locator 缺少内容", node)
        if needle not in _line_text(full):
            raise AlignmentFailure("locator-missing", f"locator 文本未出现：{needle!r}", node)
        return
    raise AlignmentFailure("locator-invalid",
                           f"locator 只支持 Lx、Lx-Ly 或 text:<片段>：{token!r}", node)


def _norm_command(value):
    """Whitespace/backslash/case-insensitive command identity for provenance."""
    return " ".join(str(value).replace("\\", "/").split()).lower()


def _check_sha(full, sha, node):
    if sha is None:
        return True
    if not isinstance(sha, str) or not SHA_RX.match(sha.strip()):
        raise AlignmentFailure("sha-invalid", f"sha256 不是 64 位十六进制：{sha!r}", node)
    h = hashlib.sha256()
    try:
        with open(full, "rb") as fh:
            for chunk in iter(lambda: fh.read(65536), b""):
                h.update(chunk)
    except OSError as e:
        raise AlignmentFailure("hash-unreadable", f"文件读不出：{e}", node)
    if h.hexdigest().lower() != sha.strip().lower():
        raise AlignmentFailure("hash-mismatch", "sha256 与磁盘内容不符", node)
    return True


def _norm_changed(root, changed):
    out = []
    for item in changed or []:
        if not isinstance(item, str) or not item.strip():
            raise AlignmentFailure("bad-change", f"changed 路径为空或不是字符串：{item!r}")
        try:
            norm, _ = _safe_rel(root, item, None, "changed")
        except AlignmentFailure as e:
            raise AlignmentFailure("bad-change", e.message)
        out.append(norm)
    return out


def _glob_match(path, patterns):
    for p in patterns:
        p = str(p).replace("\\", "/")
        if fnmatch.fnmatchcase(path, p):
            return True
    return False


def _has_glob(pattern):
    """True when a path pattern has fnmatch magic; literal paths can be intersected."""
    return any(ch in str(pattern) for ch in "*?[")


def _scope_overlap(pattern, ng_paths):
    """Conservative declared-scope overlap: literals exact, globs only vs literals."""
    pattern = str(pattern).replace("\\", "/")
    if _has_glob(pattern):
        return any(pattern == str(p).replace("\\", "/") or
                   (not _has_glob(p) and _glob_match(str(p).replace("\\", "/"), [pattern]))
                   for p in ng_paths)
    return any(_glob_match(pattern, [str(p)]) for p in ng_paths)


def _fail(report, code, message, node=None):
    report["failures"].append({"code": code, "message": message,
                              **({"node": node} if node else {})})


def _warn(report, code, message, node=None):
    report["warnings"].append({"code": code, "message": message,
                              **({"node": node} if node else {})})


def _as_list(value, node, field, report):
    if value is None:
        return []
    if not isinstance(value, list):
        _fail(report, f"{field}-invalid", f"{field} 必须是数组", node)
        return []
    return value


def _nonempty_str(value, node, what):
    return isinstance(value, str) and bool(value.strip())


def _unknown_fields(obj, allowed, node, what, report):
    if not isinstance(obj, dict):
        _fail(report, f"{what}-invalid", f"{what} 必须是对象", node)
        return False
    unknown = sorted(set(obj) - allowed)
    if unknown:
        _fail(report, "unknown-field", f"{what} 有未知字段：{unknown}", node)
    return True


def _build_report(root, manifest_path):
    return {
        "schema": SCHEMA,
        "status": "unknown",
        "exit": 3,
        "manifest": _display(root, manifest_path),
        "score": 0.0,
        "must_score": 0.0,
        "counts": {"goals": 0, "requirements": 0, "must": 0, "artifacts": 0, "evidence": 0,
                   "provenance": 0, "provenance_verified": 0,
                   "changed": 0, "changed_known": 0, "linked_changes": 0},
        "coverage": {"requirements_with_artifact": 0, "requirements_with_evidence": 0,
                     "requirements_with_provenance": 0, "requirements_provenance_verified": 0,
                     "requirements_with_changes": 0, "requirements_changes_realized": 0,
                     "requirements_full": 0, "must_full": 0, "goals_covered": 0},
        "failures": [],
        "warnings": [],
    }


def load_manifest(root, manifest_path):
    """Read and syntax-check the file. Unknown shape is a report, not a traceback."""
    report = _build_report(root, manifest_path)
    if not os.path.isfile(manifest_path):
        _fail(report, "manifest-missing", f"对齐清单不存在：{report['manifest']}")
        return None, report
    try:
        with open(manifest_path, "rb") as fh:
            doc = json.loads(fh.read().decode("utf-8-sig"))
    except (OSError, UnicodeDecodeError, ValueError) as e:
        _fail(report, "manifest-unreadable", f"对齐清单读不出或坏 JSON：{e}")
        return None, report
    if not isinstance(doc, dict):
        _fail(report, "manifest-unreadable", "对齐清单必须是 JSON 对象")
        return None, report
    if doc.get("schema") not in SCHEMAS:
        _fail(report, "schema-mismatch",
              f"schema 必须是 {' 或 '.join(repr(s) for s in SCHEMAS)}，实际 {doc.get('schema')!r}")
        return None, report
    report["schema"] = doc.get("schema")
    return doc, report


def _read_goals(doc, report):
    """Validate the goal layer and return ({goal_id: goal}, root_id).

    alignment/1 keeps the legacy single `goal` object.  alignment/2 and
    alignment/3 use `goals` with an optional `parent`, allow subgoals, and
    require exactly one root.
    """
    schema = doc.get("schema")
    if schema == SCHEMA_V1:
        if "goals" in doc:
            _fail(report, "goals-field",
                  "alignment/1 使用 goal 单对象；要子目标请改用 alignment/2 的 goals 数组")
        goal = doc.get("goal")
        if not isinstance(goal, dict):
            _fail(report, "goal-missing", "必须有一个 goal 对象")
            return {}, ""
        _unknown_fields(goal, GOAL_V1_FIELDS, "goal", "goal", report)
        goal_id = str(goal.get("id", "")).strip()
        if not goal_id:
            _fail(report, "goal-invalid", "goal.id 不能为空")
        if not _nonempty_str(goal.get("text"), goal_id or None, "goal.text"):
            _fail(report, "goal-invalid", "goal.text 不能为空", goal_id or None)
        if goal_id:
            goal["_id"] = goal_id
            report["goal"] = goal_id
            report["counts"]["goals"] = 1
        return ({goal_id: goal} if goal_id else {}), goal_id

    if schema not in (SCHEMA_V2, SCHEMA_V3):
        return {}, ""

    if "goal" in doc:
        _fail(report, "goal-field",
              f"{schema} 使用 goals 数组；单目标也写成 goals:[{{...}}]")
    raw = _as_list(doc.get("goals"), None, "goals", report)
    if not raw:
        _fail(report, "goals-missing", f"{schema} 至少需要一个 goal")
        return {}, ""

    goal_map = {}
    for i, goal in enumerate(raw):
        node = f"goal[{i}]"
        if not isinstance(goal, dict):
            _fail(report, "goal-invalid", f"{node} 必须是对象", node)
            continue
        _unknown_fields(goal, GOAL_V2_FIELDS, node, "goal", report)
        goal_id = str(goal.get("id", "")).strip()
        if not goal_id:
            _fail(report, "goal-invalid", f"{node} 缺少 id", node)
            continue
        if goal_id in goal_map:
            _fail(report, "duplicate-id", f"goal id 重复：{goal_id}", goal_id)
            continue
        if not _nonempty_str(goal.get("text"), goal_id, "goal.text"):
            _fail(report, "goal-invalid", f"{goal_id} 的 text 不能为空", goal_id)
        goal["_id"] = goal_id
        goal_map[goal_id] = goal

    report["counts"]["goals"] = len(goal_map)
    parent_of = {}
    roots = []
    for goal_id, goal in goal_map.items():
        parent = goal.get("parent")
        if parent is None:
            roots.append(goal_id)
            continue
        if not isinstance(parent, str) or not parent.strip():
            _fail(report, "goal-parent",
                  f"{goal_id} 的 parent 必须是 goal id 或 null", goal_id)
            continue
        parent = parent.strip()
        parent_of[goal_id] = parent
        if parent not in goal_map:
            _fail(report, "unknown-goal-parent",
                  f"{goal_id} 的 parent 不存在：{parent}", goal_id)

    if len(roots) != 1:
        _fail(report, "goal-root",
              f"必须有且只有一个根 goal（parent=null），实际 {len(roots)} 个：{roots}")
        root_id = ""
    else:
        root_id = roots[0]
        report["goal"] = root_id

    # Every goal must reach the unique root; cycles and unknown parents fail.
    for goal_id in goal_map:
        seen = []
        cur = goal_id
        while cur in goal_map:
            if cur in seen:
                _fail(report, "goal-cycle",
                      f"goal parent 链成环：{' -> '.join(seen + [cur])}", goal_id)
                break
            seen.append(cur)
            parent = parent_of.get(cur)
            if parent is None:
                if root_id and cur != root_id:
                    _fail(report, "goal-not-root",
                          f"{goal_id} 的 goal 链没有落到根 {root_id}（实际 {cur}）", goal_id)
                break
            cur = parent
        else:
            if cur:
                _fail(report, "goal-not-root",
                      f"{goal_id} 的 goal 链没有落到根 {root_id or '<none>'}（实际 {cur}）", goal_id)
    return goal_map, root_id


def validate(root, doc, changed=None, strict=True, executed=None, changed_known=False):
    root = os.path.abspath(root)
    report = _build_report(root, os.path.join(root, ".agent", "alignment.json"))
    if not isinstance(doc, dict):
        _fail(report, "manifest-invalid", "对齐清单必须是对象")
        return report
    # validate() must be idempotent: it annotates nodes with `_id`/`_priority`
    # while walking the graph.  Work on a copy so a second validation of the
    # same caller object cannot see its own synthetic fields as unknown keys.
    doc = copy.deepcopy(doc)
    schema = doc.get("schema", SCHEMA)
    report["schema"] = schema
    _unknown_fields(doc, TOP_FIELDS, "manifest", "manifest", report)
    evidence_fields = EV_FIELDS_V3 if schema == SCHEMA_V3 else EV_FIELDS

    goal_map, goal_id = _read_goals(doc, report)

    requirements = _as_list(doc.get("requirements"), None, "requirements", report)
    artifacts = _as_list(doc.get("artifacts"), None, "artifacts", report)
    evidence = _as_list(doc.get("evidence"), None, "evidence", report)
    non_goals = _as_list(doc.get("non_goals"), None, "non_goals", report)
    allow_unlinked = _as_list(doc.get("allow_unlinked"), None, "allow_unlinked", report)
    for i, item in enumerate(allow_unlinked):
        if not isinstance(item, str) or not item.strip():
            _fail(report, "allow-unlinked-invalid",
                  f"allow_unlinked[{i}] 必须是非空 glob 字符串", "allow_unlinked")
    for i, raw in enumerate(allow_unlinked):
        if isinstance(raw, str):
            norm = raw.replace("\\", "/")
            if os.path.isabs(raw) or CHANGE_BAD_RX.match(raw) or ".." in norm.split("/"):
                _fail(report, "allow-unlinked-invalid",
                      f"allow_unlinked[{i}] 必须是仓库相对 glob：{raw!r}", "allow_unlinked")

    if not requirements:
        _fail(report, "no-requirements", "至少需要一条 requirement；空清单不算对齐")

    # --- ids: one namespace for goal/requirements/artifacts/evidence/non_goals -----
    ids = {}
    for goal_id in goal_map:
        ids[goal_id] = "goal"
    for kind, items, spec in (("requirement", requirements, REQ_FIELDS),
                              ("artifact", artifacts, ART_FIELDS),
                              ("evidence", evidence, evidence_fields),
                              ("non_goal", non_goals, NG_FIELDS)):
        for i, item in enumerate(items):
            node = f"{kind}[{i}]"
            if not isinstance(item, dict):
                _fail(report, f"{kind}-invalid", f"{node} 必须是对象", node)
                continue
            _unknown_fields(item, spec, node, kind, report)
            item_id = str(item.get("id", "")).strip()
            if not item_id:
                _fail(report, f"{kind}-id", f"{node} 缺少 id", node)
                continue
            if item_id in ids:
                _fail(report, "duplicate-id", f"id 重复：{item_id}", item_id)
            else:
                ids[item_id] = kind
            item["_id"] = item_id
            item["_kind"] = kind

    req_map = {r.get("_id"): r for r in requirements
               if isinstance(r, dict) and r.get("_id")}
    artifact_map = {a.get("_id"): a for a in artifacts
                    if isinstance(a, dict) and a.get("_id")}
    evidence_map = {e.get("_id"): e for e in evidence
                    if isinstance(e, dict) and e.get("_id")}
    non_goal_map = {n.get("_id"): n for n in non_goals
                    if isinstance(n, dict) and n.get("_id")}

    # Explicit non-goal object references: a requirement may not cite an
    # artifact/evidence that the manifest already excluded by name.
    ng_art_refs = set()
    ng_ev_refs = set()
    for nid, ng in non_goal_map.items():
        for field, target, sink in (("artifacts", artifact_map, ng_art_refs),
                                    ("evidence", evidence_map, ng_ev_refs)):
            refs = _as_list(ng.get(field), nid, field, report)
            for ref in refs:
                if not isinstance(ref, str) or ref not in target:
                    _fail(report, f"non-goal-{field}-ref",
                          f"{nid} 引用了不存在的 {field}：{ref!r}", nid)
                    continue
                sink.add(ref)

    # --- requirement shape/parent chain ------------------------------------------
    valid_parents = set(req_map) | set(goal_map)
    parent_of = {}
    for i, req in enumerate(requirements):
        if not isinstance(req, dict):
            continue
        rid = req.get("_id") or f"requirement[{i}]"
        if not _nonempty_str(req.get("text"), rid, "requirement.text"):
            _fail(report, "requirement-text", f"{rid} 的 text 不能为空", rid)
        priority = req.get("priority", "must")
        if priority not in PRIORITIES:
            _fail(report, "priority-invalid", f"{rid} 的 priority 必须是 must/should/may", rid)
            priority = "must"
        req["_priority"] = priority
        changes = req.get("changes")
        if changes is not None and not isinstance(changes, list):
            _fail(report, "changes-invalid", f"{rid} 的 changes 必须是数组", rid)
        parent = req.get("parent")
        if not isinstance(parent, str) or not parent.strip():
            _fail(report, "missing-parent", f"{rid} 没有 parent（需求必须显式挂到目标或上层需求）", rid)
            parent = ""
        else:
            parent = parent.strip()
            parent_of[rid] = parent
            if parent not in valid_parents:
                _fail(report, "unknown-parent", f"{rid} 的 parent 不存在：{parent}", rid)

    # cycle detection: every requirement must terminate at the single goal
    for rid in req_map:
        seen = []
        cur = rid
        while cur in req_map:
            if cur in seen:
                _fail(report, "parent-cycle", f"parent 链成环：{' -> '.join(seen + [cur])}", rid)
                break
            seen.append(cur)
            cur = parent_of.get(cur, "")
        else:
            if cur not in goal_map:
                _fail(report, "goal-not-reached",
                      f"{rid} 的 parent 链没有终止在任何 goal（实际 {cur or '<empty>'}）", rid)

    # --- artifacts/evidence: path, hash, locator --------------------------------
    artifact_ok = set()
    for i, art in enumerate(artifacts):
        if not isinstance(art, dict):
            continue
        aid = art.get("_id") or f"artifact[{i}]"
        path = art.get("path")
        if not isinstance(path, str) or not path.strip():
            _fail(report, "artifact-path", f"{aid} 缺少 path", aid)
            continue
        try:
            norm, full = _is_file(root, path, aid, "artifact")
            _check_sha(full, art.get("sha256"), aid)
            _check_locator(full, art.get("locator"), aid)
            artifact_ok.add(aid)
            art["_path"] = norm
        except AlignmentFailure as e:
            _fail(report, e.code, e.message, e.node or aid)

    evidence_ok = set()
    evidence_prov = set()
    evidence_verified = set()
    executed_norm = None
    if executed is not None:
        executed_norm = {_norm_command(x) for x in executed
                         if isinstance(x, str) and x.strip()}
    for i, ev in enumerate(evidence):
        if not isinstance(ev, dict):
            continue
        eid = ev.get("_id") or f"evidence[{i}]"
        kind = ev.get("kind", "file") if schema == SCHEMA_V3 else "file"
        if schema == SCHEMA_V3 and kind not in EVIDENCE_KINDS:
            _fail(report, "evidence-kind",
                  f"{eid} 的 kind 必须是 file 或 run：{kind!r}", eid)
            continue
        path = ev.get("path")
        if not isinstance(path, str) or not path.strip():
            _fail(report, "evidence-path", f"{eid} 缺少 path", eid)
            continue
        try:
            norm, full = _is_file(root, path, eid, "evidence")
            _check_sha(full, ev.get("sha256"), eid)
            _check_locator(full, ev.get("locator"), eid)
            evidence_ok.add(eid)
            ev["_path"] = norm
        except AlignmentFailure as e:
            _fail(report, e.code, e.message, e.node or eid)
            continue
        if schema == SCHEMA_V3 and kind == "run":
            command = ev.get("command")
            if not isinstance(command, str) or not command.strip():
                _fail(report, "evidence-command",
                      f"{eid} 是 run 证据，但没有声明 command", eid)
                continue
            if norm.lower() not in _norm_command(command):
                _fail(report, "evidence-command-path",
                      f"{eid} 的 command 没有点名证据文件 {norm}", eid)
                continue
            evidence_prov.add(eid)
            if executed_norm is not None:
                if _norm_command(command) in executed_norm:
                    evidence_verified.add(eid)
                else:
                    _fail(report, "evidence-not-executed",
                          f"{eid} 声明的 command 没有出现在本次成功 run 记录里：{command!r}", eid)

    # --- non_goals must be concrete path globs ---------------------------------
    for i, ng in enumerate(non_goals):
        if not isinstance(ng, dict):
            continue
        nid = ng.get("_id") or f"non_goal[{i}]"
        if not _nonempty_str(ng.get("text"), nid, "non_goal.text"):
            _fail(report, "non-goal-text", f"{nid} 的 text 不能为空", nid)
        paths = ng.get("paths")
        if not isinstance(paths, list) or not paths:
            _fail(report, "non-goal-paths", f"{nid} 必须给出至少一个 paths glob", nid)
            ng["_paths"] = []
            continue
        clean = []
        for p in paths:
            if not isinstance(p, str) or not p.strip():
                _fail(report, "non-goal-paths", f"{nid} 的 paths 含空值", nid)
                continue
            raw = p.strip().replace("\\", "/")
            if os.path.isabs(p) or CHANGE_BAD_RX.match(p) or raw.startswith("../") \
                    or ".." in raw.split("/"):
                _fail(report, "non-goal-paths", f"{nid} 的路径必须是仓库相对 glob：{p!r}", nid)
                continue
            clean.append(raw)
        ng["_paths"] = clean

    # Declared scope must not overlap a non-goal before the run even starts.
    # Glob-vs-glob intersection is not decidable with fnmatch, so exact duplicate
    # globs and literal paths are rejected; actual changes stay the post-run net.
    for aid, art in artifact_map.items():
        apath = art.get("_path")
        if not apath:
            continue
        for nid, ng in non_goal_map.items():
            if _glob_match(apath, ng.get("_paths", [])):
                _fail(report, "non-goal-artifact-path",
                      f"{aid} 的产物路径落进 non_goal {nid}：{apath}", aid)
    for raw in allow_unlinked:
        if not isinstance(raw, str):
            continue
        for nid, ng in non_goal_map.items():
            paths = ng.get("_paths", [])
            if _scope_overlap(raw, paths):
                _fail(report, "non-goal-scope",
                      f"allow_unlinked {raw!r} 与 non_goal {nid} 重叠", "allow_unlinked")

    # --- requirement references + coverage --------------------------------------
    total_full = 0
    must_total = must_full = 0
    for i, req in enumerate(requirements):
        if not isinstance(req, dict):
            continue
        rid = req.get("_id") or f"requirement[{i}]"
        arts = _as_list(req.get("artifacts"), rid, "artifacts", report)
        evs = _as_list(req.get("evidence"), rid, "evidence", report)
        for aid in arts:
            if not isinstance(aid, str) or aid not in artifact_map:
                _fail(report, "missing-artifact-ref", f"{rid} 引用了不存在的 artifact：{aid!r}", rid)
            elif aid in ng_art_refs:
                _fail(report, "non-goal-artifact",
                      f"{rid} 引用了 non_goal 明确排除的 artifact：{aid}", rid)
        for eid in evs:
            if not isinstance(eid, str) or eid not in evidence_map:
                _fail(report, "missing-evidence-ref", f"{rid} 引用了不存在的 evidence：{eid!r}", rid)
            elif eid in ng_ev_refs:
                _fail(report, "non-goal-evidence",
                      f"{rid} 引用了 non_goal 明确排除的 evidence：{eid}", rid)
        changes = req.get("changes")
        if isinstance(changes, list):
            for p in changes:
                if not isinstance(p, str):
                    continue
                for nid, ng in non_goal_map.items():
                    paths = ng.get("_paths", [])
                    if _scope_overlap(p, paths):
                        _fail(report, "non-goal-scope",
                              f"{rid} 的 declared change 与 non_goal {nid} 重叠：{p}", rid)
        has_art = any(aid in artifact_ok for aid in arts if isinstance(aid, str))
        has_ev = any(eid in evidence_ok for eid in evs if isinstance(eid, str))
        has_prov = schema == SCHEMA_V3 and any(
            eid in evidence_prov for eid in evs if isinstance(eid, str))
        has_verified = schema == SCHEMA_V3 and any(
            eid in evidence_verified for eid in evs if isinstance(eid, str))
        priority = req.get("_priority", "must")
        if has_art:
            report["coverage"]["requirements_with_artifact"] += 1
        if has_ev:
            report["coverage"]["requirements_with_evidence"] += 1
        if has_prov:
            report["coverage"]["requirements_with_provenance"] += 1
        if has_verified:
            report["coverage"]["requirements_provenance_verified"] += 1
        if has_art and has_ev:
            total_full += 1
        if priority == "must":
            must_total += 1
            if has_art and has_ev:
                must_full += 1
            if not has_art:
                _fail(report, "missing-artifact", f"{rid} 是 must，但没有有效产物", rid)
            if not has_ev:
                _fail(report, "missing-evidence", f"{rid} 是 must，但没有有效证据", rid)
            if schema == SCHEMA_V3 and not has_prov:
                _fail(report, "missing-provenance",
                      f"{rid} 是 must，但没有绑定 run command 的证据", rid)
        elif priority == "should":
            if strict and not has_art:
                _fail(report, "missing-artifact", f"{rid} 是 should（strict），但没有有效产物", rid)
            elif not has_art:
                _warn(report, "missing-artifact", f"{rid} 是 should，但没有有效产物", rid)
            if strict and not has_ev:
                _fail(report, "missing-evidence", f"{rid} 是 should（strict），但没有有效证据", rid)
            elif not has_ev:
                _warn(report, "missing-evidence", f"{rid} 是 should，但没有有效证据", rid)
            if schema == SCHEMA_V3 and not has_prov:
                if strict:
                    _fail(report, "missing-provenance",
                          f"{rid} 是 should（strict），但没有绑定 run command 的证据", rid)
                else:
                    _warn(report, "missing-provenance",
                          f"{rid} 是 should，但没有绑定 run command 的证据", rid)
        else:
            if not has_art:
                _warn(report, "optional-no-artifact", f"{rid} 是 may，没有产物（仅警告）", rid)
            if not has_ev:
                _warn(report, "optional-no-evidence", f"{rid} 是 may，没有证据（仅警告）", rid)
            if schema == SCHEMA_V3 and not has_prov:
                _warn(report, "optional-no-provenance",
                      f"{rid} 是 may，没有绑定 run command 的证据（仅警告）", rid)
        req["_artifacts"] = arts
        req["_evidence"] = evs

    # --- contradictions ----------------------------------------------------------
    pairs = set()
    for i, req in enumerate(requirements):
        if not isinstance(req, dict):
            continue
        rid = req.get("_id") or f"requirement[{i}]"
        for other in _as_list(req.get("contradicts"), rid, "contradicts", report):
            if not isinstance(other, str) or other not in req_map:
                _fail(report, "unknown-contradiction", f"{rid} 声明矛盾的对象不存在：{other!r}", rid)
                continue
            if other == rid:
                _fail(report, "self-contradiction", f"{rid} 声明与自己矛盾", rid)
                continue
            pair = tuple(sorted((rid, other)))
            if pair in pairs:
                continue
            pairs.add(pair)
            _fail(report, "contradiction", f"需求互相矛盾：{pair[0]} <-> {pair[1]}",
                  f"{pair[0]}|{pair[1]}")

    # --- changed files: every change must align to a requirement or artifact ------
    # `changed` alone is a caller-supplied coverage set (it may be a subset);
    # only `changed_known` says the full actual set is known, so only that can
    # prove every declared change actually happened.
    coverage_known = changed is not None or changed_known
    full_known = changed_known
    changed_norm = []
    if coverage_known:
        try:
            changed_norm = _norm_changed(root, changed or [])
        except AlignmentFailure as e:
            _fail(report, e.code, e.message)
            changed_norm = []
        report["counts"]["changed_known"] = 1 if full_known else 0
        report["counts"]["changed"] = len(changed_norm)
        linked = set()
        for path in changed_norm:
            hit_ng = [nid for nid, ng in non_goal_map.items()
                      if _glob_match(path, ng.get("_paths", []))]
            if hit_ng:
                _fail(report, "non-goal-change",
                      f"变更落进 non_goal {hit_ng[0]}：{path}", hit_ng[0])
                continue
            if _glob_match(path, allow_unlinked):
                linked.add(path)
                continue
            hit_req = []
            for rid, req in req_map.items():
                if _glob_match(path, req.get("changes", []) if isinstance(req.get("changes"), list) else []):
                    hit_req.append(rid)
            hit_art = [aid for aid, art in artifact_map.items() if art.get("_path") == path]
            if hit_req or hit_art:
                linked.add(path)
            else:
                _fail(report, "unlinked-change", f"变更没有对齐到任何 requirement：{path}")
        report["counts"]["linked_changes"] = len(linked)
        # A declared change is a promise: only the full actual set can witness it.
        # An empty set is not a free pass (the old no-op smoke proved that hole);
        # must/strict failures, may warns.
        if full_known:
            for rid, req in req_map.items():
                patterns = req.get("changes") if isinstance(req.get("changes"), list) else []
                if not patterns:
                    continue
                report["coverage"]["requirements_with_changes"] += 1
                missing = [str(p) for p in patterns
                           if not any(_glob_match(path, [str(p)]) for path in changed_norm)]
                if not missing:
                    report["coverage"]["requirements_changes_realized"] += 1
                    continue
                message = f"{rid} 声明的变更在本次实际变更中不存在：{', '.join(missing)}"
                priority = req.get("_priority", "must")
                if priority == "must" or (priority == "should" and strict):
                    _fail(report, "declared-change-missing", message, rid)
                else:
                    _warn(report, "declared-change-missing", message, rid)

    report["counts"].update({"requirements": len(req_map), "artifacts": len(artifact_ok),
                             "evidence": len(evidence_ok),
                             "provenance": len(evidence_prov),
                             "provenance_verified": len(evidence_verified)})
    report["counts"]["must"] = must_total
    report["coverage"].update({"requirements_full": total_full, "must_full": must_full,
                               "goals_covered": 1 if goal_id and req_map else 0})
    total_req = len(req_map)
    report["score"] = round(total_full / total_req, 3) if total_req else 0.0
    report["must_score"] = round(must_full / must_total, 3) if must_total else 0.0

    if report["failures"]:
        report["status"] = "unaligned"
        report["exit"] = 1
    else:
        report["status"] = "aligned"
        report["exit"] = 0
    return report


def check(root, manifest, changed=None, strict=True, executed=None, changed_known=False):
    root = os.path.abspath(root)
    manifest_abs = manifest if os.path.isabs(manifest) else os.path.join(root, manifest)
    root_real = os.path.realpath(root)
    manifest_real = os.path.realpath(os.path.abspath(manifest_abs))
    try:
        common = os.path.commonpath([root_real, manifest_real])
    except ValueError:
        common = ""
    if os.path.normcase(common) != os.path.normcase(root_real):
        report = _build_report(root, manifest_real)
        _fail(report, "manifest-outside-root",
              f"对齐清单路径必须留在项目 root 内：{report['manifest']}")
        return report
    doc, report = load_manifest(root, manifest_real)
    if doc is None:
        return report
    got = validate(root, doc, changed, strict, executed=executed,
                   changed_known=changed_known)
    got["manifest"] = report["manifest"]
    if report["failures"]:
        # syntax-level failures (schema/bad JSON) are known-unreadable, not aligned
        got["status"] = "unknown"
        got["exit"] = 3
        got["failures"] = report["failures"] + got["failures"]
    return got


def _say(text):
    try:
        print(text)
    except UnicodeEncodeError:
        enc = getattr(sys.stdout, "encoding", None) or "utf-8"
        print(text.encode(enc, "replace").decode(enc, "replace"))


def _emit(report, as_json, out_path):
    if out_path:
        os.makedirs(os.path.dirname(os.path.abspath(out_path)), exist_ok=True)
        with open(out_path, "w", encoding="ascii", newline="\n") as fh:
            json.dump(report, fh, ensure_ascii=True, sort_keys=True)
            fh.write("\n")
    if as_json:
        _say(json.dumps(report, ensure_ascii=True, sort_keys=True))
        return
    c = report["counts"]
    cov = report["coverage"]
    _say(f"alignment: {report['status']} (score={report['score']:.3f}, "
         f"must={report['must_score']:.3f}) goal={report.get('goal', '-')} "
         f"goals={c['goals']} req={c['requirements']} must={c['must']} artifacts={c['artifacts']} "
         f"evidence={c['evidence']} changed={c['changed']} linked={c['linked_changes']}")
    _say(f"  coverage: artifact={cov['requirements_with_artifact']}/{c['requirements']} "
         f"evidence={cov['requirements_with_evidence']}/{c['requirements']} "
         f"full={cov['requirements_full']}/{c['requirements']} "
         f"must_full={cov['must_full']}/{c['must']}")
    if report.get("schema") == SCHEMA_V3:
        _say(f"  provenance: declared={c['provenance']} verified={c['provenance_verified']} "
             f"req_declared={cov['requirements_with_provenance']}/{c['requirements']} "
             f"req_verified={cov['requirements_provenance_verified']}/{c['requirements']}")
    for f in report["failures"]:
        _say(f"  E {f['code']}: {f['message']}")
    for w in report["warnings"]:
        _say(f"  W {w['code']}: {w['message']}")


def run_check(args):
    root = os.path.abspath(args.root or os.getcwd())
    manifest = args.manifest or os.path.join(root, ".agent", "alignment.json")
    if not os.path.isabs(manifest):
        manifest = os.path.join(root, manifest)
    changed = []
    for item in args.changed:
        changed.extend([x.strip() for x in item.split(",") if x.strip()])
    known = bool(changed) or bool(getattr(args, "changed_set", False))
    report = check(root, manifest, changed if known else None, not args.lenient,
                   executed=args.run_command or None,
                   changed_known=bool(getattr(args, "changed_set", False)))
    _emit(report, args.json, args.out)
    return report["exit"]


def _scaffold(args):
    """Write a deliberately incomplete alignment/2 skeleton.

    The skeleton names one root goal and the requirement texts, but leaves
    artifacts/evidence/changes empty on purpose: the gate must still refuse
    until the caller does the thinking and fills the graph.  This is a
    checklist generator, not an auto-alignment claim.
    """
    root = os.path.abspath(args.root or os.getcwd())
    goal = str(getattr(args, "goal", "") or "").strip()
    requirements = [str(x).strip() for x in (getattr(args, "requirement", []) or []) if str(x).strip()]
    if not goal:
        _say("alignment_gate scaffold: --goal 不能为空")
        return 2
    if not requirements:
        _say("alignment_gate scaffold: 至少给一个 --requirement（重复传参）")
        return 2
    out = str(getattr(args, "manifest", "") or ".agent/alignment.json")
    try:
        rel = os.path.relpath(out, root) if os.path.isabs(out) else out
        _norm, full = _safe_rel(root, rel, None, "manifest")
    except (AlignmentFailure, ValueError) as e:
        _say(f"alignment_gate scaffold: {getattr(e, 'message', str(e))}")
        return 2
    if os.path.isdir(full):
        _say(f"alignment_gate scaffold: 目标是目录而不是文件：{_display(root, full)}")
        return 2
    if os.path.exists(full) and not getattr(args, "force", False):
        _say(f"alignment_gate scaffold: 已存在，未覆盖（要覆盖加 --force）：{_display(root, full)}")
        return 2
    doc = {
        "schema": SCHEMA_V2,
        "goals": [{"id": "G1", "text": goal}],
        "requirements": [
            {"id": f"R{i + 1}", "text": text, "priority": "must", "parent": "G1",
             "artifacts": [], "evidence": [], "changes": []}
            for i, text in enumerate(requirements)
        ],
        "artifacts": [],
        "evidence": [],
        "non_goals": [],
        "allow_unlinked": [],
        "notes": "scaffold: artifacts/evidence/changes 故意留空；补齐前 gate 不会 aligned",
    }
    try:
        os.makedirs(os.path.dirname(full), exist_ok=True)
        tmp = full + ".tmp"
        with open(tmp, "w", encoding="utf-8", newline="\n") as fh:
            json.dump(doc, fh, ensure_ascii=False, indent=2)
            fh.write("\n")
        os.replace(tmp, full)
    except OSError as e:
        _say(f"alignment_gate scaffold: 写不出 {_display(root, full)}：{e}")
        return 2
    _say(f"scaffold: wrote {_display(root, full)} (goal=1, requirements={len(requirements)}, "
         "artifacts/evidence/changes=empty -> gate will refuse until filled)")
    return 0


# ---------------------------------------------------------------------------
# self-test: every assertion is a contract the gate must not lose
# ---------------------------------------------------------------------------

def _write(root, rel, text):
    full = os.path.join(root, rel.replace("/", os.sep))
    os.makedirs(os.path.dirname(full), exist_ok=True)
    with open(full, "w", encoding="utf-8", newline="\n") as fh:
        fh.write(text)
    return full


def _base_doc():
    return {
        "schema": SCHEMA,
        "goal": {"id": "G1", "text": "把输入整理成可验收的产物"},
        "requirements": [
            {"id": "R1", "text": "产物包含目标字段", "priority": "must",
             "parent": "G1", "artifacts": ["A1"], "evidence": ["E1"],
             "changes": ["src/a.py"]},
            {"id": "R2", "text": "证据能定位到测试", "priority": "should",
             "parent": "G1", "artifacts": ["A1"], "evidence": ["E1"]},
        ],
        "artifacts": [{"id": "A1", "path": "src/a.py"}],
        "evidence": [{"id": "E1", "path": "tests/test_a.py", "locator": "text:test_a"}],
        "non_goals": [{"id": "N1", "text": "不碰 vendor", "paths": ["vendor/**"]}],
        "allow_unlinked": ["Cargo.lock"],
    }


def _base_v2_doc():
    doc = _base_doc()
    doc["schema"] = SCHEMA_V2
    del doc["goal"]
    doc["goals"] = [{"id": "G1", "text": "把输入整理成可验收的产物", "parent": None}]
    return doc


def _base_v3_doc():
    doc = _base_v2_doc()
    doc["schema"] = SCHEMA_V3
    doc["evidence"] = [{"id": "E1", "path": "tests/test_a.py", "locator": "text:test_a",
                        "kind": "run", "command": "python -m pytest tests/test_a.py"}]
    return doc


def _selftest():
    checks = 0

    def ok(cond, msg):
        nonlocal checks
        checks += 1
        if not cond:
            raise AssertionError(msg)

    def status(root, doc, changed=None, strict=True, executed=None, changed_known=False):
        return validate(root, doc, changed, strict, executed=executed,
                        changed_known=changed_known)

    def expect(report, code, msg):
        ok(any(f["code"] == code for f in report["failures"]), f"{msg}（实际 {report['failures']}）")

    with tempfile.TemporaryDirectory() as td:
        root = os.path.join(td, "root")
        os.makedirs(root)
        _write(root, "src/a.py", "def f():\n    return 1\n")
        _write(root, "tests/test_a.py", "from src.a import f\n\ndef test_a():\n    assert f() == 1\n")
        base = _base_doc()

        r = status(root, base)
        ok(r["status"] == "aligned" and r["exit"] == 0, "有效清单 aligned")
        ok(r["score"] == 1.0 and r["must_score"] == 1.0, "覆盖率 1.0")
        ok(r["coverage"]["goals_covered"] == 1, "goal 覆盖")
        ok(r["coverage"]["requirements_full"] == 2, "两条需求全链")
        ok(r["counts"]["goals"] == 1, "v1 单目标计数")

        d = _base_v2_doc()
        r = status(root, d)
        ok(r["status"] == "aligned" and r["schema"] == SCHEMA_V2, "v2 单根目标 aligned")
        r = check(root, _write(root, "v2.json", json.dumps(_base_v2_doc(), ensure_ascii=False)))
        ok(r["status"] == "aligned" and r["schema"] == SCHEMA_V2, "v2 清单经 check/load 仍 aligned")
        d = _base_v2_doc()
        d["goals"].append({"id": "G2", "text": "子目标", "parent": "G1"})
        d["requirements"][0]["parent"] = "G2"
        r = status(root, d)
        ok(r["status"] == "aligned" and r["counts"]["goals"] == 2, "v2 子目标链 aligned")
        d = _base_v2_doc()
        d["goals"].append({"id": "G2", "text": "子目标", "parent": "G1"})
        d["goals"][0]["parent"] = "G2"
        expect(status(root, d), "goal-cycle", "v2 goal 成环")
        d = _base_v2_doc()
        d["goals"].append({"id": "G2", "text": "第二个根", "parent": None})
        expect(status(root, d), "goal-root", "v2 多个根")
        d = _base_v2_doc()
        d["goals"][0]["parent"] = "G9"
        expect(status(root, d), "unknown-goal-parent", "v2 未知 goal parent")
        d = _base_v2_doc()
        del d["goals"]
        expect(status(root, d), "goals-missing", "v2 缺 goals")
        d = _base_v2_doc()
        d["goal"] = {"id": "G1", "text": "混用 field"}
        expect(status(root, d), "goal-field", "v2 混用 goal 字段")
        d = _base_doc()
        d["goals"] = [{"id": "G1", "text": "混用 field"}]
        expect(status(root, d), "goals-field", "v1 混用 goals 字段")

        d = _base_v3_doc()
        r = status(root, d)
        ok(r["status"] == "aligned" and r["schema"] == SCHEMA_V3, "v3 声明清单 aligned")
        ok(r["counts"]["provenance"] == 1 and r["counts"]["provenance_verified"] == 0,
           "v3 preflight 只记 declared provenance")
        r = status(root, d, executed=["python -m pytest tests/test_a.py"])
        ok(r["status"] == "aligned" and r["counts"]["provenance_verified"] == 1,
           "v3 成功 run command 证明证据")
        ok(r["coverage"]["requirements_provenance_verified"] == 2,
           "v3 两条需求的证据都经实际命令验证")
        r = status(root, d, executed=["pytest -q"])
        expect(r, "evidence-not-executed", "v3 声明的 command 实际没跑")
        r = status(root, d, executed=["python -m pytest tests/test_a.py -q"])
        expect(r, "evidence-not-executed", "v3 command 只允许精确匹配")
        d = _base_v3_doc(); d["evidence"][0]["kind"] = "file"
        expect(status(root, d), "missing-provenance", "v3 must 不能只给静态文件证据")
        d = _base_v3_doc(); d["evidence"][0]["command"] = "python -m pytest"
        expect(status(root, d), "evidence-command-path", "v3 command 必须点名证据文件")
        d = _base_v3_doc(); d["evidence"][0]["command"] = ""
        expect(status(root, d), "evidence-command", "v3 run 证据缺 command")
        d = _base_v3_doc(); d["evidence"][0]["kind"] = "shell"
        expect(status(root, d), "evidence-kind", "v3 非法 evidence kind")
        d = _base_v3_doc(); d["evidence"][0]["receipt"] = "run.json"
        expect(status(root, d), "unknown-field", "v3 evidence 不接受未建模字段")
        d = _base_v3_doc()
        d["requirements"] = [{"id": "R1", "text": "只做可选", "priority": "may",
                              "parent": "G1", "artifacts": ["A1"], "evidence": ["E1"]}]
        d["evidence"][0]["kind"] = "file"
        r = status(root, d)
        ok(r["status"] == "aligned" and any(w["code"] == "optional-no-provenance"
                                             for w in r["warnings"]),
           "v3 may 静态证据只警告缺 provenance")
        d = _base_v3_doc(); d["goal"] = {"id": "G1", "text": "混用"}
        expect(status(root, d), "goal-field", "v3 混用 goal 字段")
        d = _base_v2_doc(); d["evidence"][0]["kind"] = "run"
        expect(status(root, d), "unknown-field", "v2 不接受 v3 evidence 字段")

        d = _base_doc(); del d["goal"]
        expect(status(root, d), "goal-missing", "缺 goal")
        d = _base_doc(); d["goal"]["text"] = ""
        expect(status(root, d), "goal-invalid", "空 goal text")
        d = _base_doc(); d["requirements"][1]["id"] = "R1"
        expect(status(root, d), "duplicate-id", "重复 id")
        d = _base_doc(); d["requirements"][0]["parent"] = "G9"
        expect(status(root, d), "unknown-parent", "未知 parent")
        d = _base_doc(); d["requirements"][0]["parent"] = "R2"; d["requirements"][1]["parent"] = "R1"
        expect(status(root, d), "parent-cycle", "parent 成环")
        d = _base_doc(); d["requirements"] = []
        expect(status(root, d), "no-requirements", "空需求")
        d = _base_doc(); d["requirements"][0]["artifacts"] = ["A9"]
        expect(status(root, d), "missing-artifact-ref", "悬空 artifact ref")
        d = _base_doc(); d["requirements"][0]["evidence"] = ["E9"]
        expect(status(root, d), "missing-evidence-ref", "悬空 evidence ref")
        d = _base_doc(); d["requirements"][0]["artifacts"] = []
        expect(status(root, d), "missing-artifact", "must 缺产物")
        d = _base_doc(); d["requirements"][0]["evidence"] = []
        expect(status(root, d), "missing-evidence", "must 缺证据")
        d = _base_doc(); d["requirements"][1]["evidence"] = []
        r = status(root, d, strict=True)
        expect(r, "missing-evidence", "strict 下 should 缺证据")
        r = status(root, d, strict=False)
        ok(r["status"] == "aligned" and any(w["code"] == "missing-evidence" for w in r["warnings"]),
           "lenient 下 should 缺证据只警告")
        d = _base_doc(); d["requirements"][0]["priority"] = "may"; d["requirements"][0]["artifacts"] = []
        r = status(root, d)
        ok(r["status"] == "aligned" and any(w["code"] == "optional-no-artifact" for w in r["warnings"]),
           "may 缺产物只警告")

        d = _base_doc(); d["artifacts"][0]["path"] = "src/missing.py"
        expect(status(root, d), "artifact-missing", "产物文件缺失")
        d = _base_doc(); d["evidence"][0]["path"] = "tests/missing.py"
        expect(status(root, d), "evidence-missing", "证据文件缺失")
        d = _base_doc(); d["artifacts"][0]["sha256"] = "0" * 64
        expect(status(root, d), "hash-mismatch", "哈希不符")
        good_sha = hashlib.sha256(open(os.path.join(root, "src/a.py"), "rb").read()).hexdigest()
        d = _base_doc(); d["artifacts"][0]["sha256"] = good_sha.upper()
        ok(status(root, d)["status"] == "aligned", "哈希大小写等价")
        d = _base_doc(); d["evidence"][0]["locator"] = "L1-L4"
        ok(status(root, d)["status"] == "aligned", "行号 locator 命中")
        d = _base_doc(); d["evidence"][0]["locator"] = "L99"
        expect(status(root, d), "locator-out-of-range", "行号越界")
        d = _base_doc(); d["evidence"][0]["locator"] = "text:not-here"
        expect(status(root, d), "locator-missing", "文本 locator 未命中")
        d = _base_doc(); d["evidence"][0]["locator"] = "## section"
        expect(status(root, d), "locator-invalid", "未知 locator 语法")

        d = _base_doc(); d["requirements"][0]["changes"] = ["src/other.py"]
        expect(status(root, d, changed=["misc/x.py"]), "unlinked-change", "变更未对齐")
        d = _base_doc(); d["requirements"][0]["changes"] = ["lib/**"]
        ok(status(root, d, changed=["lib/x.py"])["status"] == "aligned", "glob 对齐变更")
        d = _base_doc(); d["requirements"][0]["changes"] = []
        d["allow_unlinked"] = ["generated/**"]
        ok(status(root, d, changed=["generated/x.py"])["status"] == "aligned", "allow_unlinked")
        d = _base_doc(); d["non_goals"][0]["paths"] = ["src/**"]
        expect(status(root, d, changed=["src/a.py"]), "non-goal-change", "变更落进 non_goal")
        d = _base_doc(); d["requirements"][0]["changes"] = ["src/a.py"]
        r = status(root, d, changed=["src/a.py"], changed_known=True)
        ok(r["status"] == "aligned" and r["coverage"]["requirements_changes_realized"] == 1,
           "匹配的 declared change 才算已实现")
        d = _base_doc(); d["requirements"][0]["changes"] = ["src/old.py"]
        r = status(root, d, changed=["src/a.py"], changed_known=True)
        expect(r, "declared-change-missing", "声明的变更没有实际发生")
        d = _base_doc(); d["requirements"][0]["changes"] = ["src/old.py"]
        r = status(root, d, changed=["src/a.py"])
        ok(r["status"] == "aligned" and not any(f["code"] == "declared-change-missing"
                                                for f in r["failures"]),
           "preflight 的 --changed 是覆盖集，不强制声明变更全部兑现")
        d = _base_doc(); d["requirements"][0]["changes"] = ["src/a.py"]
        ok(status(root, d)["status"] == "aligned", "preflight 无实际变更集时只声明")
        expect(status(root, d, changed=[], changed_known=True),
               "declared-change-missing", "空实际变更集不是免检通行证")
        r = status(root, d, changed=[], changed_known=True)
        ok(r["counts"]["changed_known"] == 1 and r["counts"]["changed"] == 0,
           "changed_known 区分空集与未提供")
        d = _base_doc(); d["requirements"][0]["changes"] = ["src/**"]
        ok(status(root, d, changed=["src/a.py"], changed_known=True)["status"] == "aligned",
           "glob 覆盖实际变更")
        d = _base_doc(); d["requirements"][0]["changes"] = ["src/a.py", "src/old.py"]
        expect(status(root, d, changed=["src/a.py"], changed_known=True),
               "declared-change-missing", "一个声明命中不能掩盖另一个声明缺失")
        d = _base_doc(); d["requirements"][0]["changes"] = []
        d["requirements"][1]["changes"] = ["src/old.py"]
        expect(status(root, d, changed=[], changed_known=True), "declared-change-missing",
               "strict 下 should 的声明变更缺失也拒绝")
        d = _base_doc(); d["requirements"][0]["changes"] = []
        d["requirements"][1]["changes"] = ["src/old.py"]
        r = status(root, d, changed=[], changed_known=True, strict=False)
        ok(r["status"] == "aligned" and
           any(w["code"] == "declared-change-missing" for w in r["warnings"]),
           "lenient 下 should 声明变更缺失只警告")
        d = _base_doc()
        expect(status(root, d, changed=["../escape.py"]), "bad-change", "changed 逃逸")

        d = _base_doc(); d["requirements"][0]["contradicts"] = ["R2"]
        expect(status(root, d), "contradiction", "需求矛盾")
        d = _base_doc(); d["requirements"][0]["contradicts"] = ["R9"]
        expect(status(root, d), "unknown-contradiction", "矛盾对象不存在")
        d = _base_doc(); d["artifacts"][0]["path"] = "../escape.py"
        expect(status(root, d), "bad-path", ".. 逃逸")
        d = _base_doc(); d["artifacts"][0]["path"] = "C:/abs.py"
        expect(status(root, d), "bad-path", "绝对路径")
        d = _base_doc(); d["schema"] = "alignment/9"
        r = check(root, _write(root, "m1.json", json.dumps(d, ensure_ascii=False)))
        ok(r["status"] == "unknown" and r["exit"] == 3, "错误 schema -> unknown")
        _write(root, "bad.json", "{")
        r = check(root, os.path.join(root, "bad.json"))
        ok(r["status"] == "unknown" and r["exit"] == 3, "坏 JSON -> unknown")
        r = check(root, os.path.join(root, "none.json"))
        ok(r["status"] == "unknown" and r["exit"] == 3, "缺口清单 -> unknown")
        outside = _write(td, "outside.json", json.dumps(_base_doc(), ensure_ascii=False))
        r = check(root, outside)
        ok(r["status"] == "unknown" and any(f["code"] == "manifest-outside-root"
                                            for f in r["failures"]),
           "root 外的清单 -> unknown")
        d = _base_doc(); d["requirements"][0]["priority"] = "required"
        expect(status(root, d), "priority-invalid", "非法优先级")
        d = _base_doc(); d["requirements"][0]["typo"] = True
        expect(status(root, d), "unknown-field", "未知字段")
        d = _base_doc(); d["requirements"][0]["text"] = ""
        expect(status(root, d), "requirement-text", "空需求文本")
        d = _base_doc(); d["artifacts"][0]["sha256"] = "xyz"
        expect(status(root, d), "sha-invalid", "非法哈希格式")
        d = _base_doc(); d["requirements"][0]["changes"] = "src/a.py"
        expect(status(root, d), "changes-invalid", "changes 非数组")
        d = _base_doc(); d["non_goals"][0]["paths"] = []
        expect(status(root, d), "non-goal-paths", "non_goal 无路径")
        d = _base_doc(); d["non_goals"][0]["artifacts"] = ["A1"]
        expect(status(root, d), "non-goal-artifact", "requirement 引用 non_goal artifact")
        d = _base_doc(); d["non_goals"][0]["evidence"] = ["E1"]
        expect(status(root, d), "non-goal-evidence", "requirement 引用 non_goal evidence")
        d = _base_doc(); d["non_goals"][0]["artifacts"] = ["A9"]
        expect(status(root, d), "non-goal-artifacts-ref", "non_goal 引用未知 artifact")
        d = _base_doc(); d["non_goals"][0]["evidence"] = ["E9"]
        expect(status(root, d), "non-goal-evidence-ref", "non_goal 引用未知 evidence")
        d = _base_doc()
        d["artifacts"].append({"id": "A2", "path": "src/extra.py"})
        _write(root, "src/extra.py", "x = 1\n")
        d["non_goals"][0]["artifacts"] = ["A2"]
        r = status(root, d)
        ok(r["status"] == "aligned", "未被 requirement 引用的 non_goal artifact 不误报")
        _write(root, "vendor/a.py", "x = 1\n")
        d = _base_doc(); d["artifacts"][0]["path"] = "vendor/a.py"
        expect(status(root, d), "non-goal-artifact-path", "产物路径落进 non_goal")
        d = _base_doc(); d["requirements"][0]["changes"] = ["vendor/x.py"]
        expect(status(root, d), "non-goal-scope", "declared literal change 落进 non_goal")
        d = _base_doc(); d["requirements"][0]["changes"] = ["vendor/**"]
        expect(status(root, d), "non-goal-scope", "declared glob 与 non_goal 完全相同")
        d = _base_doc(); d["non_goals"][0]["paths"] = ["vendor/a.py"]
        d["requirements"][0]["changes"] = ["vendor/**"]
        expect(status(root, d), "non-goal-scope", "declared glob 覆盖 non_goal 字面路径")
        d = _base_doc(); d["allow_unlinked"] = ["vendor/**"]
        expect(status(root, d), "non-goal-scope", "allow_unlinked glob 与 non_goal 重叠")
        d = _base_doc(); d["allow_unlinked"] = ["vendor/x.py"]
        expect(status(root, d), "non-goal-scope", "allow_unlinked literal 落进 non_goal")

        scaffold = os.path.join(root, "scaffold.json")
        args = argparse.Namespace(root=root, manifest="scaffold.json", goal="对齐骨架",
                                  requirement=["需求一", "需求二"], force=False)
        ok(_scaffold(args) == 0 and os.path.isfile(scaffold), "scaffold 写出清单")
        sdoc = json.loads(open(scaffold, encoding="utf-8").read())
        ok(sdoc.get("schema") == SCHEMA_V2 and len(sdoc.get("goals", [])) == 1
           and len(sdoc.get("requirements", [])) == 2, "scaffold 形状 v2/goal/requirements")
        r = check(root, scaffold, changed=["src/a.py"])
        expect(r, "missing-evidence", "scaffold 未填证据仍拒绝")
        expect(r, "unlinked-change", "scaffold 未填 changes 仍拒绝")
        before = open(scaffold, encoding="utf-8").read()
        ok(_scaffold(args) == 2 and open(scaffold, encoding="utf-8").read() == before,
           "scaffold 不覆盖已有清单")
        args.force = True
        ok(_scaffold(args) == 0, "scaffold --force 覆盖")
        args = argparse.Namespace(root=root, manifest="../escape.json", goal="逃逸",
                                  requirement=["需求"], force=True)
        ok(_scaffold(args) == 2 and not os.path.exists(os.path.join(td, "escape.json")),
           "scaffold 拒绝 root 外路径")
        args = argparse.Namespace(root=root, manifest="empty.json", goal="没有需求",
                                  requirement=[], force=True)
        ok(_scaffold(args) == 2 and not os.path.exists(os.path.join(root, "empty.json")),
           "scaffold 没需求不落盘")

        r1 = status(root, _base_doc(), ["src/a.py"])
        r2 = status(root, _base_doc(), ["src/a.py"])
        ok(json.dumps(r1, sort_keys=True) == json.dumps(r2, sort_keys=True),
           "同输入输出确定")

    _say(f"alignment_gate self-test: {checks}/{checks}")
    return 0


def _stress(cases):
    errors = 0
    aligned = 0
    rejected = 0
    rng = random.Random(20261124)
    with tempfile.TemporaryDirectory() as td:
        for i in range(cases):
            root = os.path.join(td, f"v{i}")
            os.makedirs(root)
            _write(root, "src/a.py", f"def f():\n    return {i}\n")
            _write(root, "tests/test_a.py", "def test_a():\n    assert True\n")
            d = {
                "schema": SCHEMA,
                "goal": {"id": "G1", "text": "压力用例"},
                "requirements": [],
                "artifacts": [{"id": "A1", "path": "src/a.py"}],
                "evidence": [{"id": "E1", "path": "tests/test_a.py"}],
                "non_goals": [{"id": "N1", "text": "不碰 vendor", "paths": ["vendor/**"]}],
            }
            for n in range(rng.randint(1, 4)):
                d["requirements"].append({
                    "id": f"R{n}", "text": f"需求 {n}", "priority": "must",
                    "parent": "G1", "artifacts": ["A1"], "evidence": ["E1"],
                    "changes": ["src/a.py"],
                })
            r = validate(root, d)
            if r["status"] == "aligned":
                aligned += 1
            else:
                errors += 1
                _say(f"stress valid case {i} unexpectedly {r['status']}: {r['failures']}")

            # mutation: guaranteed reject without touching the file set
            mut = json.loads(json.dumps(d))
            mode = rng.randrange(3)
            if mode == 0:
                mut["requirements"][0]["evidence"] = []
            elif mode == 1:
                mut["requirements"][0]["changes"] = ["unmapped.py"]
            else:
                mut["non_goals"][0]["paths"] = ["src/**"]
            changed = ["misc/x.py"] if mode == 1 else ["src/a.py"]
            r2 = validate(root, mut, changed)
            if r2["status"] == "unaligned":
                rejected += 1
            else:
                errors += 1
                _say(f"stress mutated case {i} unexpectedly {r2['status']}: {r2['failures']}")
    _say(f"alignment_gate stress: cases={cases * 2} valid_aligned={aligned} "
         f"mutated_rejected={rejected} errors={errors}")
    return 0 if errors == 0 else 1


def main(argv=None):
    argv = list(sys.argv[1:] if argv is None else argv)
    parser = argparse.ArgumentParser(prog="alignment_gate")
    parser.add_argument("cmd", nargs="?", default="check",
                        choices=("check", "self-test", "stress", "scaffold"))
    parser.add_argument("--root", default="")
    parser.add_argument("--manifest", default="")
    parser.add_argument("--changed", action="append", default=[])
    parser.add_argument("--changed-set", action="store_true",
                        help="actual changed set is known, even when it is empty")
    parser.add_argument("--run-command", action="append", default=[])
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--lenient", action="store_true")
    parser.add_argument("--out", default="")
    parser.add_argument("--cases", type=int, default=100)
    parser.add_argument("--goal", default="")
    parser.add_argument("--requirement", action="append", default=[])
    parser.add_argument("--force", action="store_true")
    args = parser.parse_args(argv)
    if args.cmd == "self-test":
        return _selftest()
    if args.cmd == "stress":
        if args.cases <= 0:
            _say("alignment_gate: --cases must be > 0")
            return 2
        return _stress(args.cases)
    if args.cmd == "scaffold":
        return _scaffold(args)
    return run_check(args)


if __name__ == "__main__":
    raise SystemExit(main())
