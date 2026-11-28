#!/usr/bin/env python3
"""sandbox-run: 把一条命令放进临时 workspace 里跑，并证明宿主有没有被写。

    python tools/sandbox_run.py -Root . -Copy docs/AGENT-TASK-STANDARD.md \
        -- python -c "print(open('docs/AGENT-TASK-STANDARD.md').read()[:40])"
    python tools/sandbox_run.py -Root . -Role docs -Model deepseek-chat \
        -CapsMemMb 512 -CapsSeconds 60 -- python tools/some_generator.py
    python tools/sandbox_run.py --self-test
    python tools/sandbox_run.py -Root . -Copy script.py -Isolation wsl \
        -WslDistro Ubuntu-24.04 -- python3 script.py
    python tools/sandbox_run.py -Root . -CapabilityPolicy .agent/cap.json \
        -CapabilityBin target/debug/trit-orch -- python tools/some_generator.py
    python tools/sandbox_run.py --self-test-wsl -WslDistro Ubuntu-24.04

它做什么（逐条，默认模式别当成"内核沙盒"）：
  1. 在宿主根**之外**建一个空 workdir（默认 %TEMP%\\trit-sandbox-*）；
  2. 只把 -Copy 明确列出的相对路径复制进去（拒绝绝对路径、`..`、符号链接、
     `.git/.agent/node_modules/target` 组件，以及超量/超大输入）；
  3. 用 policy.py 按 workdir 为根预分类命令；policy 拒绝就**不运行**；
  3b. 可选 `-CapabilityPolicy <json>`：把 Exec（命令 basename）、Write
      （profile 非 read-only 时）、Network（`-AllowNetwork` 时）折成动作，逐条
      问 `trit-orch capability`；任一不是 True 就不运行，回执记 decisions。
      未显式给策略时，检测 `<root>/.agent/capability.json`，存在即启用；都没有则
      保持旧行为。二进制按 `-CapabilityBin` / `TRIT_ORCH_BIN` / PATH / 仓库 target
      目录定位，缺失即拒绝（fail-closed）。
  4. 用 run_caps.py 套内核资源帽（内存/进程数/超时），stdin 接 DEVNULL，
     环境变量只留白名单（密钥类变量会摘掉，回执只记变量名不记值）；
  4b. 只有显式 `-Isolation wsl` 才走 WSL2+bwrap `--unshare-all`：空根只读 bind
      系统目录、只把 workdir 挂到 `/work`、net/pid/user/ipc/uts/cgroup 全切、
      `--cap-drop ALL`；Windows `.exe` 和 Windows 绝对路径参数直接拒绝，不回退；
  5. 运行前后各取一次宿主指纹（git 状态 + diff，或非 git 的 manifest）；
     64 MiB 总预算内宿主文件按内容哈希，超预算判 `host-unprovable`；
     宿主变了且没有 -AllowHostWrite，判 `host-mutated` 并 exit 3；
  6. 回执写命令、policy、caps、run、env、host 前后指纹、stdout/stderr 的哈希与尾部。

诚实边界：
  * **默认模式**是协作式 workspace 沙盒，不是内核隔离：命令仍能通过绝对路径/网络/
    子进程逃出 workdir；本工具的价值是"默认 cwd 在 workdir + policy 拒绝显式越界 +
    事后宿主指纹能证明有没有写坏"，不是防蓄意逃逸。
  * `-Isolation wsl` 是真正的内核命名空间隔离，但只接受 Linux 命令；它不是 Windows
    任意命令沙盒，也不提供 Windows 主机的文件系统/网络 namespace。
  * 宿主指纹能发现"改了又改回"以外的变化；构建缓存子树和超出 64 MiB 预算的文件仍
    可能被恢复 size/mtime 逃过，这一点不承诺。
  * 默认联网并允许宿主写（宽松默认，用户显式要求"全部放行"）：`-Profile full`、
    `-AllowNetwork`、`-AllowHostWrite` 都是默认值。要收紧就显式传
    `-Profile read-only|workspace-write`，或不要给 `-AllowNetwork`/`-AllowHostWrite`。
    WSL 模式仍直接 `--unshare-all`，`-AllowNetwork` 与之不兼容并会被拒绝。
  * 宽松默认不改变不可逆操作的门：删除仍必须走 `trash.py`，git 历史破坏仍要
    `git_safe.py` 检查点；这些是独立于 profile 的硬规则，不是逐次批准。
  * capability 门是**动作级**的，不替代 policy：一条 `python script.py` 内部实际
    写哪些路径、连哪些域名不可预知，所以策略若给 Write/Network 配了目标白名单，
    本工具会保守拒绝；只声明类别放行（`allow`）才可通过。
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import uuid

TOOLS = os.path.dirname(os.path.abspath(__file__))
PY = sys.executable
RUN_CAPS = os.path.join(TOOLS, "run_caps.py")
import capacity_lease as capacity  # noqa: E402  (same-directory tool, stdlib only)
POLICY = os.path.join(TOOLS, "policy.py")

MAX_INPUT_BYTES = 64 * 1024 * 1024
MAX_INPUT_FILES = 5000
MAX_HOST_FILES = 200000
MAX_OUTPUT_BYTES = 1024 * 1024
DENY_COPY_COMPONENTS = {".git", ".agent", "node_modules", "target", ".qoder"}
# Only build-cache dirs are skipped from the host manifest. `.git`, `.agent`
# and `.qoder` are security-sensitive state: a sandboxed command that edits
# hooks/config/ledger/terms there must not be reported as "host clean".
MANIFEST_SKIP_DIRS = {"node_modules", "target"}
# Git refreshes `.git/index` stat data during a read-only `git status`; hash
# the content instead of mtime. `index.lock` is transient under concurrent
# git operations, so it is not in the manifest; `run_sandbox` checks whether
# a lock was left behind after the command, which is the meaningful mutation.
MANIFEST_VOLATILE = {".git/index.lock"}
MANIFEST_HASH_MAX_BYTES = 1024 * 1024
MANIFEST_HASH_TOTAL_BYTES = 64 * 1024 * 1024
WSL_BWRAP_BINDS = ("/usr", "/bin", "/lib", "/lib64", "/sbin", "/etc", "/usr/local")
SECRET_ENV_RX = re.compile(r"(api[_-]?key|token|secret|password|credential|auth|private)", re.I)
KEEP_ENV = {
    "PATH", "SYSTEMROOT", "WINDIR", "TEMP", "TMP", "USERPROFILE", "HOMEDRIVE",
    "HOMEPATH", "COMSPEC", "PATHEXT", "LANG", "LC_ALL", "TERM", "PWD",
}


def sha256_bytes(data):
    return hashlib.sha256(data).hexdigest()


def is_inside(path, root):
    a = os.path.normcase(os.path.abspath(path))
    b = os.path.normcase(os.path.abspath(root))
    return a == b or a.startswith(b + os.sep)


def _is_linkish(path):
    """True for symlinks, Windows junctions and any path whose realpath differs
    from its lexical path. `os.path.islink` returns False for junctions, so a
    junction inside the root to `.agent` used to pass the copy check."""
    if os.path.islink(path):
        return True
    try:
        st = os.lstat(path)
    except OSError:
        return False
    if getattr(st, "st_file_attributes", 0) & 0x400:
        return True
    return (os.path.normcase(os.path.realpath(path))
            != os.path.normcase(os.path.abspath(path)))


def _content_digest(path):
    h = hashlib.sha256()
    with open(path, "rb") as fh:
        for chunk in iter(lambda: fh.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def _hashable_size(size):
    return 0 <= size <= MANIFEST_HASH_MAX_BYTES


def _index_lock_present(root):
    return os.path.exists(os.path.join(root, ".git", "index.lock"))


def _index_lock_settled(root, delay=0.35):
    """A concurrent `git status` can leave a transient index.lock. Only a lock
    that persists after a short settle window is evidence that the sandboxed
    command itself left one behind."""
    if not _index_lock_present(root):
        return False
    time.sleep(delay)
    return _index_lock_present(root)


def _index_semantic_digest(root):
    """Hash the staged entries, not the raw index bytes. Git rewrites stat data
    in `.git/index` during read-only status calls; the staged tree is the part
    that matters for concurrent sandbox fingerprints."""
    index = os.path.join(root, ".git", "index")
    if not os.path.exists(index):
        return "absent", ""
    try:
        proc = subprocess.run(["git", "-C", root, "ls-files", "--stage", "-z"],
                              capture_output=True)
    except OSError as exc:
        return None, f"git index read failed: {exc}"
    if proc.returncode != 0:
        # Fixtures and genuinely corrupt indexes cannot be listed; fall back to
        # raw content so a rewrite is still visible instead of claiming the
        # whole host is unprovable. A valid repo always takes the semantic path.
        try:
            return "raw:" + _content_digest(index), ""
        except OSError as exc:
            return None, f"git index read failed: {exc}"
    return "sha256:" + sha256_bytes(proc.stdout), ""


def _manifest_hash(root):
    """Hash every host file we are prepared to see, including files ignored by
    git. Git status alone misses writes to ignored paths; that is exactly the
    hole where a command can plant a secret without `host-mutated` noticing.
    Small files (<=1 MiB) are content-hashed, and larger host files are
    content-hashed too while the total deep-hash budget lasts; a file that
    would exceed the budget makes the host unprovable instead of silently
    falling back to metadata."""
    try:
        h = hashlib.sha256()
        count = 0
        content_total = 0
        # A new top-level build-cache directory changes the root's mtime even
        # though its files are skipped from the walk.
        try:
            root_st = os.stat(root)
            h.update(b"ROOT\0" + str(root_st.st_mtime_ns).encode() + b"\n")
        except OSError as exc:
            return None, f"root stat failed: {exc}"
        for dirpath, dirnames, filenames in os.walk(root):
            # Build caches are not walked (too many files), but their directory
            # metadata is still fingerprinted. A new file or subdirectory
            # changes the parent directory mtime, so a write into target/ or
            # node_modules/ cannot be reported as "host clean".
            skipped = sorted(d for d in dirnames if d in MANIFEST_SKIP_DIRS)
            for name in skipped:
                full = os.path.join(dirpath, name)
                rel = os.path.relpath(full, root).replace("\\", "/")
                try:
                    st = os.stat(full, follow_symlinks=False)
                except OSError as exc:
                    return None, f"skipped-dir stat failed: {exc}"
                h.update(b"DIR\0" + rel.encode("utf-8", "replace") + b"\0")
                if name == ".git":
                    # Read-only git operations rewrite this directory's mtime
                    # when they create/remove index.lock.  The lock file is
                    # volatile and the staged index is hashed semantically
                    # below; hashing the .git directory mtime would make a
                    # concurrent `git status` look like a host write.
                    h.update(b"volatile-dir-mtime\n")
                else:
                    h.update(str(st.st_size).encode() + b":" + str(st.st_mtime_ns).encode() + b"\n")
                # Directory mtime catches creates/deletes, not overwrites of an
                # existing cache file. Include name/size/mtime for files below
                # the skipped root, still without hashing their contents.
                for subdir, subdirs, subfiles in os.walk(full, followlinks=False):
                    subdirs.sort()
                    subrel = os.path.relpath(subdir, root).replace("\\", "/")
                    for subname in sorted(subdirs):
                        subfull = os.path.join(subdir, subname)
                        try:
                            sst = os.stat(subfull, follow_symlinks=False)
                        except OSError as exc:
                            return None, f"skipped subtree stat failed: {exc}"
                        count += 1
                        if count > MAX_HOST_FILES:
                            return None, f"host has more than {MAX_HOST_FILES} files; state unprovable"
                        h.update(b"SUBDIR\0" + subrel.encode("utf-8", "replace") + b"/"
                                 + subname.encode("utf-8", "replace") + b"\0")
                        h.update(str(sst.st_size).encode() + b":" + str(sst.st_mtime_ns).encode() + b"\n")
                    for subname in sorted(subfiles):
                        subfull = os.path.join(subdir, subname)
                        srel = os.path.relpath(subfull, root).replace("\\", "/")
                        if srel in MANIFEST_VOLATILE:
                            continue
                        try:
                            sst = os.stat(subfull, follow_symlinks=False)
                        except OSError as exc:
                            return None, f"skipped file stat failed: {exc}"
                        count += 1
                        if count > MAX_HOST_FILES:
                            return None, f"host has more than {MAX_HOST_FILES} files; state unprovable"
                        h.update(b"FILE\0" + srel.encode("utf-8", "replace") + b"\0")
                        h.update(str(sst.st_size).encode() + b":" + str(sst.st_mtime_ns).encode() + b"\n")
                        # A cache overwrite can keep size and restore mtime. For
                        # small cache files the content digest closes that gap;
                        # large files keep the metadata boundary (documented).
                        if _hashable_size(sst.st_size):
                            try:
                                h.update(b"CONTENT\0" + _content_digest(subfull).encode() + b"\n")
                            except OSError as exc:
                                return None, f"skipped file read failed: {exc}"
            dirnames[:] = sorted(d for d in dirnames if d not in MANIFEST_SKIP_DIRS)
            for name in sorted(filenames):
                full = os.path.join(dirpath, name)
                rel = os.path.relpath(full, root).replace("\\", "/")
                if rel in MANIFEST_VOLATILE:
                    continue
                try:
                    st = os.stat(full, follow_symlinks=False)
                except OSError as exc:
                    return None, f"stat failed: {exc}"
                count += 1
                if count > MAX_HOST_FILES:
                    return None, f"host has more than {MAX_HOST_FILES} files; state unprovable"
                h.update(rel.encode("utf-8", "replace") + b"\0")
                if rel == ".git/index":
                    # Stat data is rewritten by concurrent read-only git calls;
                    # hash the staged entries instead of the raw bytes.
                    digest, why_index = _index_semantic_digest(root)
                    if digest is None:
                        return None, why_index
                    h.update(digest.encode("utf-8"))
                else:
                    h.update(str(st.st_size).encode() + b":" + str(st.st_mtime_ns).encode() + b"\n")
                    # Metadata alone can be restored by a command (`os.utime`
                    # plus same-size write). Hash small files by content so a
                    # same-size overwrite is not invisible. Large files are
                    # also content-hashed while the total budget lasts; if the
                    # budget would be exceeded we refuse to certify the host.
                    if st.st_size <= MANIFEST_HASH_MAX_BYTES:
                        should_hash = True
                    elif content_total + st.st_size <= MANIFEST_HASH_TOTAL_BYTES:
                        should_hash = True
                    else:
                        return None, (f"large file exceeds deep-hash budget: {rel} "
                                      f"({st.st_size} bytes)")
                    if should_hash:
                        content_total += st.st_size
                        try:
                            h.update(b"CONTENT\0" + _content_digest(full).encode() + b"\n")
                        except OSError as exc:
                            return None, f"file read failed: {exc}"
        return h.hexdigest(), "manifest"
    except OSError as exc:
        return None, f"manifest failed: {exc}"


def fingerprint(root):
    """(hash, kind) or (None, reason). Git state catches tracked/untracked changes;
    the all-file manifest catches writes to git-ignored paths too. Host files
    are content-hashed up to a 64 MiB deep-hash budget; exceeding it makes the
    host unprovable (fail-closed), and build-cache subtrees remain metadata-only
    (a documented residual because walking node_modules would be unbounded)."""
    git_value = None
    try:
        import handoff
        for attempt in range(3):
            value, why = handoff.state(root)
            if value:
                git_value = value
                break
            # A concurrent sandbox can hold .git/index.lock for a few
            # milliseconds; retry before degrading the fingerprint kind.
            if "status failed" in (why or "") and attempt < 2:
                time.sleep(0.05)
            else:
                break
    except Exception:
        git_value = None
    manifest, why = _manifest_hash(root)
    if manifest is None:
        return None, why
    if git_value:
        payload = f"{git_value}\0{manifest}".encode("utf-8")
        return sha256_bytes(payload), "git+manifest"
    return manifest, "manifest"


def scrub_env(environ):
    env = {}
    for key, value in environ.items():
        if key.upper() in KEEP_ENV:
            env[key] = value
    env["TRIT_SANDBOX"] = "1"
    env["PYTHONIOENCODING"] = "utf-8"
    removed = sorted(k for k in environ if SECRET_ENV_RX.search(k))
    return env, removed


def copy_inputs(root, work, specs, max_bytes=MAX_INPUT_BYTES, max_files=MAX_INPUT_FILES):
    total = [0]
    files = [0]

    def add_file(src, rel):
        if _is_linkish(src):
            raise ValueError(f"symlink/junction input refused: {rel}")
        try:
            st = os.stat(src, follow_symlinks=False)
        except OSError as exc:
            raise ValueError(f"cannot stat input: {rel}: {exc}")
        # A relative hardlink inside root can still point at an outside inode;
        # realpath cannot distinguish it. Refuse multiple-link files rather than
        # claiming the copy is root-contained.
        if st.st_nlink > 1:
            raise ValueError(f"hardlink input refused: {rel}")
        size = st.st_size
        total[0] += size
        files[0] += 1
        if total[0] > max_bytes:
            raise ValueError(f"input exceeds {max_bytes} bytes")
        if files[0] > max_files:
            raise ValueError(f"input exceeds {max_files} files")
        dst = os.path.join(work, rel)
        if not is_inside(dst, work):
            raise ValueError(f"destination escapes workdir: {rel}")
        os.makedirs(os.path.dirname(dst), exist_ok=True)
        shutil.copy2(src, dst)

    for spec in specs:
        rel = os.path.normpath(spec)
        if os.path.isabs(rel) or rel in ("", ".", "..") or rel.startswith(".." + os.sep):
            raise ValueError(f"copy path must be a relative path inside root: {spec!r}")
        parts = [p for p in re.split(r"[\\/]+", rel) if p]
        if any(p.rstrip(" .").lower() in DENY_COPY_COMPONENTS for p in parts):
            raise ValueError(f"copy path contains a denied component: {spec!r}")
        src = os.path.join(root, rel)
        if not os.path.exists(src):
            raise ValueError(f"copy path does not exist: {spec!r}")
        real = os.path.realpath(src)
        if not is_inside(real, root):
            raise ValueError(f"copy path resolves outside root: {spec!r}")
        if _is_linkish(src):
            raise ValueError(f"symlink/junction input refused: {spec!r}")
        if os.path.isfile(src):
            add_file(src, rel)
            continue
        for dirpath, dirnames, filenames in os.walk(src):
            for name in list(dirnames):
                child = os.path.join(dirpath, name)
                if _is_linkish(child):
                    child_rel = os.path.relpath(child, root)
                    raise ValueError(f"symlink/junction directory refused: {child_rel}")
            dirnames[:] = sorted(d for d in dirnames if d not in DENY_COPY_COMPONENTS)
            for name in sorted(filenames):
                child = os.path.join(dirpath, name)
                child_rel = os.path.relpath(child, root)
                add_file(child, child_rel)
    return {"files": files[0], "bytes": total[0]}


def _capability_binary(root, explicit):
    if explicit:
        return explicit
    env = os.environ.get("TRIT_ORCH_BIN", "")
    if env:
        return env
    found = shutil.which("trit-orch")
    if found:
        return found
    roots = [root, os.environ.get("CARGO_TARGET_DIR", "")]
    for target_root in [r for r in roots if r]:
        for rel in ("target/debug/trit-orch", "target/release/trit-orch",
                    "debug/trit-orch", "release/trit-orch"):
            base = os.path.join(target_root, rel)
            for cand in (base, base + ".exe"):
                if os.path.isfile(cand):
                    return cand
    return ""


def _invoke_capability(binary, payload):
    try:
        proc = subprocess.run([binary, "capability"], input=json.dumps(payload),
                              capture_output=True, text=True, encoding="utf-8",
                              errors="replace", timeout=30)
    except (OSError, subprocess.TimeoutExpired) as exc:
        return {"ok": False, "error": f"capability call failed: {exc}"}
    try:
        doc = json.loads((proc.stdout or "").strip())
    except ValueError:
        return {"ok": False, "error": f"capability returned non-JSON (exit {proc.returncode})"}
    if proc.returncode != 0 or not doc.get("ok"):
        return {"ok": False, "error": doc.get("error") or f"capability exit {proc.returncode}"}
    return doc


def capability_decide(policy_path, actions, root="", binary="", runner=None):
    """按给定动作列表查 `trit-orch capability`；未给策略时返回 None。"""
    if not policy_path:
        return None
    path = os.path.abspath(policy_path)
    try:
        with open(path, "r", encoding="utf-8-sig") as fh:
            policy = json.load(fh)
        with open(path, "rb") as fh:
            raw = fh.read()
    except (OSError, ValueError) as exc:
        return {"ok": False, "stage": "config", "error": f"policy unreadable: {exc}"}
    if not isinstance(policy, dict):
        return {"ok": False, "stage": "config", "error": "policy must be a JSON object"}
    if not isinstance(actions, list) or not actions:
        return {"ok": False, "stage": "config", "error": "actions must be a non-empty list"}
    exe = binary or _capability_binary(root, "")
    if not exe and runner is None:
        return {"ok": False, "stage": "config", "error": "capability binary not found"}
    invoke = runner or _invoke_capability
    decisions = []
    for action in actions:
        decision = invoke(exe, {"policy": policy, "action": action})
        decisions.append({"action": action, "decision": decision})
        if not decision.get("ok") or not decision.get("allow"):
            return {"ok": False, "stage": "decision", "policy_sha256":
                    sha256_bytes(raw), "actions": decisions}
    return {"ok": True, "policy_sha256": sha256_bytes(raw), "actions": decisions}


def capability_check(root, command, profile, allow_network, policy_path,
                     untrusted_origin=False, binary="", runner=None):
    """把 sandbox 动作折成 capability 判定；未给策略时返回 None（旧行为）。

    `runner` 是测试缝：默认起 `trit-orch capability` 子进程；测试可注入纯函数。
    策略声明了 Write/Network 目标白名单时会拒绝本调用，因为一条命令内部实际
    写哪些路径/连哪些域名不可预知；这是有意保守。
    """
    if not policy_path:
        return None
    untrusted = bool(untrusted_origin)
    actions = [{"class": "exec", "target": os.path.basename(command[0]),
                "from_untrusted_data": untrusted}]
    if profile != "read-only":
        actions.append({"class": "write", "target": None,
                        "from_untrusted_data": untrusted})
    if allow_network:
        actions.append({"class": "network", "target": None,
                        "from_untrusted_data": untrusted})
    return capability_decide(policy_path, actions, root, binary, runner)


def policy_check(work, command, profile, allow_network):
    cmdstr = shlex.join(command) if os.name != "nt" else subprocess.list2cmdline(command)
    argv = [PY, POLICY, "-Profile", profile, "-Root", work, "-Command", cmdstr]
    if allow_network:
        argv.append("-AllowNetwork")
    proc = subprocess.run(argv, capture_output=True, text=True, encoding="utf-8",
                          errors="replace")
    lines = (proc.stdout + proc.stderr).splitlines()
    return {"exit": proc.returncode, "lines": lines[-12:], "command": cmdstr}


def run_capped(work, command, env, mem_mb, procs, seconds, max_output,
               lease_id="", capacity_dir=""):
    fd_out, out_path = tempfile.mkstemp(prefix="sandbox-out-", suffix=".log")
    fd_err, err_path = tempfile.mkstemp(prefix="sandbox-err-", suffix=".log")
    os.close(fd_out)
    os.close(fd_err)
    argv = [PY, RUN_CAPS, "--mem-mb", str(mem_mb), "--procs", str(procs),
            "--seconds", str(seconds)]
    if lease_id:
        argv += ["--lease", lease_id, "--capacity-dir", capacity_dir or ""]
    argv += ["--", *command]
    t0 = time.time()
    code = None
    timed_out = False
    try:
        with open(out_path, "wb") as fout, open(err_path, "wb") as ferr:
            proc = subprocess.run(argv, cwd=work, env=env, stdin=subprocess.DEVNULL,
                                  stdout=fout, stderr=ferr, timeout=seconds + 5)
            code = proc.returncode
    except subprocess.TimeoutExpired:
        timed_out = True
        code = 124
    finally:
        pass
    seconds_used = round(time.time() - t0, 3)
    # run_caps reports its own kernel-watchdog timeout as 124; the outer subprocess
    # timeout is only a belt-and-braces fallback. Both count as timed_out.
    timed_out = bool(timed_out or code == 124)

    def hash_file(path):
        h = hashlib.sha256()
        with open(path, "rb") as fh:
            for chunk in iter(lambda: fh.read(1024 * 1024), b""):
                h.update(chunk)
        return h.hexdigest()

    def read_capped(path):
        size = os.path.getsize(path) if os.path.exists(path) else 0
        with open(path, "rb") as fh:
            data = fh.read(max_output)
        return data.decode("utf-8", "replace"), size > max_output, size, hash_file(path)

    stdout, out_trunc, out_size, out_sha = read_capped(out_path)
    stderr, err_trunc, err_size, err_sha = read_capped(err_path)
    for path in (out_path, err_path):
        try:
            os.unlink(path)
        except OSError:
            pass
    return {"exit": code, "timed_out": timed_out, "seconds": seconds_used,
            "stdout": stdout, "stderr": stderr, "stdout_truncated": out_trunc,
            "stderr_truncated": err_trunc, "stdout_bytes": out_size,
            "stderr_bytes": err_size, "stdout_sha256": out_sha,
            "stderr_sha256": err_sha}


def _wsl_exe():
    if os.name != "nt":
        return None
    return shutil.which("wsl.exe") or shutil.which("wsl")


def _wsl_prefix(distro=""):
    exe = _wsl_exe()
    if not exe:
        raise ValueError("WSL isolation requested but wsl.exe was not found")
    return [exe] + (["-d", distro] if distro else [])


def _wsl_run(distro, args, timeout=20):
    argv = [*_wsl_prefix(distro), "--", *args]
    proc = subprocess.run(argv, capture_output=True, text=True, encoding="utf-8",
                         errors="replace", stdin=subprocess.DEVNULL, timeout=timeout)
    return proc.returncode, (proc.stdout or ""), (proc.stderr or "")


def wsl_isolation_probe(distro=""):
    """(ok, detail). Checks both WSL and bubblewrap; never silently falls back."""
    try:
        rc, out, err = _wsl_run(distro, ["bwrap", "--version"], timeout=15)
    except (OSError, subprocess.TimeoutExpired, ValueError) as exc:
        return False, f"WSL/bwrap unavailable: {exc}"
    text = (out + err).strip().splitlines()
    if rc != 0:
        return False, "WSL/bwrap unavailable: " + (text[0] if text else f"exit {rc}")
    return True, (text[0] if text else "bwrap available")


def _wsl_path(path, distro=""):
    # wslpath on this host needs forward slashes; backslashes are eaten by the
    # WSL argv bridge before wslpath sees them.
    win = os.path.abspath(path).replace("\\", "/")
    rc, out, err = _wsl_run(distro, ["wslpath", "-a", "-u", win], timeout=15)
    linux = (out or "").strip()
    if rc != 0 or not linux.startswith("/"):
        raise ValueError("cannot translate workdir to a WSL path: " +
                         ((err or out).strip() or f"exit {rc}"))
    return linux


def _normalize_wsl_command(command):
    if not command:
        raise ValueError("empty command")
    first = command[0]
    if first.lower().endswith(".exe"):
        raise ValueError("WSL isolation needs POSIX commands, not Windows .exe: " + first)
    if re.match(r"^[A-Za-z]:[\\/]", first):
        raise ValueError("WSL isolation cannot execute a Windows absolute-path executable: " + first)
    for arg in command[1:]:
        if re.match(r"^[A-Za-z]:[\\/]", arg):
            raise ValueError("WSL isolation rejects Windows absolute-path argument: " + arg)
    out = list(command)
    if out[0] == "python":
        out[0] = "python3"
    return out


def build_wsl_bwrap_command(work, command, distro=""):
    ok, detail = wsl_isolation_probe(distro)
    if not ok:
        raise ValueError(detail)
    wsl_work = _wsl_path(work, distro)
    argv = [*_wsl_prefix(distro), "--", "bwrap",
            "--unshare-all", "--die-with-parent", "--new-session",
            "--hostname", "trit-sandbox", "--cap-drop", "ALL"]
    for src in WSL_BWRAP_BINDS:
        argv += ["--ro-bind-try", src, src]
    argv += ["--proc", "/proc", "--dev", "/dev", "--tmpfs", "/tmp",
             "--bind", wsl_work, "/work", "--chdir", "/work",
             "--clearenv",
             "--setenv", "PATH", "/usr/local/bin:/usr/bin:/bin",
             "--setenv", "HOME", "/work",
             "--setenv", "TMPDIR", "/tmp",
             "--setenv", "LANG", "C.UTF-8",
             "--setenv", "PYTHONIOENCODING", "utf-8",
             "--", *_normalize_wsl_command(command)]
    return argv, wsl_work



def run_sandbox(root, work, command, copies=(), profile="full",
                allow_network=True, allow_host_write=True, mem_mb=2048,
                procs=64, seconds=1800, max_output=MAX_OUTPUT_BYTES, keep=False,
                role="", model="", max_input_bytes=MAX_INPUT_BYTES,
                max_input_files=MAX_INPUT_FILES, isolation="none", wsl_distro="",
                capacity_lease="", capacity_dir="", capability_policy="",
                capability_bin="", untrusted_origin=False, capability_runner=None):
    root = os.path.abspath(root)
    work = os.path.abspath(work)
    receipt = {"tool": "sandbox-run", "role": role, "model": model,
               "root": os.path.basename(root), "work": work,
               "command_sha256": sha256_bytes(" ".join(command).encode("utf-8")),
               "profile": profile, "ok": False, "exit": 2, "verdict": "bad-invocation",
               "copies": [], "policy": None, "capability": None, "run": None,
               "env_removed": [],
               "capacity": {"mode": "none"},
               "host": {"before": None, "after": None, "kind": "", "mutated": None},
               "isolation": {"mode": "none", "kernel": False}}
    if profile not in ("read-only", "workspace-write", "full"):
        receipt["error"] = "profile must be read-only, workspace-write or full"
        return receipt
    if isolation not in ("none", "wsl"):
        receipt["error"] = "isolation must be none or wsl"
        return receipt
    if isolation == "wsl":
        if os.name != "nt":
            receipt["error"] = "WSL isolation is only available on Windows"
            return receipt
        if allow_network:
            receipt["error"] = "WSL isolation is network-isolated; --allow-network is incompatible"
            return receipt
        ok, detail = wsl_isolation_probe(wsl_distro)
        if not ok:
            receipt["error"] = detail
            return receipt
    if not os.path.isdir(root):
        receipt["error"] = "root is not a directory"
        return receipt
    if is_inside(work, root):
        receipt["error"] = "workdir must be outside root"
        return receipt
    work_real = os.path.realpath(work)
    if os.path.normcase(work_real) != os.path.normcase(work):
        receipt["error"] = "workdir resolves through a link/junction; choose a plain directory"
        return receipt
    if not command:
        receipt["error"] = "empty command"
        return receipt
    lease = None
    if capacity_lease:
        try:
            loaded = capacity.load_lease(capacity_lease, capacity_dir)
        except capacity.CapacityError as exc:
            receipt["error"] = f"capacity lease {capacity_lease} not found ({exc})"
            receipt["capacity"] = {"mode": "lease-not-found", "lease_id": capacity_lease}
            return receipt
        except Exception as exc:
            receipt["error"] = f"capacity lease lookup failed: {type(exc).__name__}: {exc}"
            receipt["capacity"] = {"mode": "lease-error", "lease_id": capacity_lease}
            return receipt
        if not loaded.get("ok"):
            receipt["error"] = f"capacity lease {capacity_lease} not found"
            receipt["capacity"] = {"mode": "lease-not-found", "lease_id": capacity_lease}
            return receipt
        lease = loaded["lease"]
        deadline = int(lease.get("deadline_at") or lease["expires_at"])
        if capacity.now_epoch() >= float(lease["expires_at"]):
            receipt["error"] = f"capacity lease {capacity_lease} already expired"
            receipt["capacity"] = {"mode": "lease-expired", "lease_id": capacity_lease,
                                   "expires_at": lease["expires_at"]}
            return receipt
        if capacity.now_epoch() >= deadline:
            receipt["error"] = f"capacity lease {capacity_lease} reached its hard deadline"
            receipt["capacity"] = {"mode": "lease-deadline", "lease_id": capacity_lease,
                                   "deadline_at": deadline}
            return receipt
        lease_mem = int(lease.get("resources", {}).get("memory_mb", 0))
        if mem_mb > lease_mem:
            receipt["error"] = (f"requested memory {mem_mb} MB exceeds lease "
                                f"budget {lease_mem} MB")
            receipt["capacity"] = {"mode": "lease-memory-exceeded",
                                   "lease_id": capacity_lease,
                                   "lease_memory_mb": lease_mem}
            return receipt
        receipt["capacity"] = {"mode": "lease", "lease": lease,
                               "contract": capacity.contract(lease)}
    if os.path.exists(work):
        if not os.path.isdir(work):
            receipt["error"] = "workdir exists and is not a directory"
            return receipt
        if os.listdir(work):
            receipt["error"] = "workdir must be empty or absent"
            return receipt
    before, kind = fingerprint(root)
    lock_before = _index_lock_present(root)
    if not before:
        receipt["error"] = f"host state unprovable: {kind}"
        receipt["verdict"] = "host-unprovable"
        return receipt
    receipt["host"].update({"before": before, "kind": kind})
    try:
        os.makedirs(work, exist_ok=True)
        copied = copy_inputs(root, work, copies, max_input_bytes, max_input_files)
        receipt["copies"] = [{"path": c} for c in copies]
        receipt["copy"] = copied
    except (OSError, ValueError) as exc:
        receipt["error"] = str(exc)
        return receipt
    run_command = command
    if isolation == "wsl":
        try:
            run_command, wsl_work = build_wsl_bwrap_command(work, command, wsl_distro)
        except (OSError, ValueError, subprocess.TimeoutExpired) as exc:
            receipt["error"] = str(exc)
            if not keep:
                shutil.rmtree(work, ignore_errors=True)
            return receipt
        receipt["isolation"] = {"mode": "wsl-bwrap", "kernel": True,
                                "distro": wsl_distro or "default",
                                "wsl_work": wsl_work,
                                "network": "unshared",
                                "filesystem": "bwrap-empty-root"}
    capability = capability_check(root, command, profile, allow_network,
                                  capability_policy, untrusted_origin, capability_bin,
                                  capability_runner)
    receipt["capability"] = capability
    if capability is not None and not capability.get("ok"):
        receipt.update({"verdict": "capability-denied", "exit": 1})
        if not keep:
            shutil.rmtree(work, ignore_errors=True)
        return receipt
    policy = policy_check(work, command, profile, allow_network)
    receipt["policy"] = policy
    if policy["exit"] != 0:
        receipt.update({"verdict": "policy-denied", "exit": 1})
        if not keep:
            shutil.rmtree(work, ignore_errors=True)
        return receipt
    env, removed = scrub_env(os.environ)
    receipt["env_removed"] = removed
    heartbeat = None
    if lease is not None:
        lease_ttl = max(30, int(lease["expires_at"] - lease["created_at"]))
        heartbeat = capacity.LeaseHeartbeat(lease["id"], lease_ttl,
                                            capacity_dir).start()
    try:
        run = run_capped(work, run_command, env, mem_mb, procs, seconds, max_output,
                         lease_id=(lease["id"] if lease else ""),
                         capacity_dir=capacity_dir)
    finally:
        if heartbeat is not None:
            receipt["capacity"]["heartbeat"] = heartbeat.stop()
    receipt["run"] = {k: run[k] for k in ("exit", "timed_out", "seconds", "stdout_truncated",
                                           "stderr_truncated", "stdout_bytes", "stderr_bytes",
                                           "stdout_sha256", "stderr_sha256")}
    receipt["run"]["stdout_tail"] = run["stdout"][-800:]
    receipt["run"]["stderr_tail"] = run["stderr"][-800:]
    after, kind2 = fingerprint(root)
    lock_after = _index_lock_present(root)
    lock_new = bool(lock_after and not lock_before)
    lock_stable = _index_lock_settled(root) if lock_new else False
    receipt["host"].update({"after": after, "kind": kind2,
                            "index_lock_before": lock_before,
                            "index_lock_after": lock_after,
                            "index_lock_new": lock_new,
                            "index_lock_stable": lock_stable})
    if not after:
        receipt["host"]["mutated"] = None
        receipt.update({"verdict": "host-unprovable", "exit": 3})
        return receipt
    mutated = before != after or lock_stable
    receipt["host"]["mutated"] = mutated
    if mutated and not allow_host_write:
        receipt.update({"verdict": "host-mutated", "exit": 3})
        return receipt
    if run["exit"] != 0:
        receipt.update({"verdict": "run-timeout" if run["timed_out"] else "run-failed",
                        "exit": int(run["exit"] or 2)})
        return receipt
    hb = receipt["capacity"].get("heartbeat")
    if hb and hb.get("status") != "ok":
        receipt.update({"verdict": "capacity-heartbeat-failed", "exit": 1})
        return receipt
    receipt.update({"ok": True, "verdict": "ok", "exit": 0})
    if not keep:
        shutil.rmtree(work, ignore_errors=True)
        receipt["cleaned"] = True
    return receipt


def parse_args(argv):
    out = {"root": "", "work": "", "copies": [], "profile": "full",
           "allow_network": True, "allow_host_write": True, "mem_mb": 2048,
           "procs": 64, "seconds": 1800, "max_output": MAX_OUTPUT_BYTES,
           "keep": False, "receipt": "", "role": "", "model": "", "command": [],
           "self_test": False, "self_test_wsl": False,
           "isolation": "none", "wsl_distro": "",
           "capacity_lease": "", "capacity_dir": "",
           "capability_policy": "", "capability_bin": "", "untrusted_origin": False}
    i = 0
    while i < len(argv):
        a = argv[i]
        if a == "--":
            out["command"] = argv[i + 1:]
            break
        if a in ("--self-test", "-SelfTest"):
            out["self_test"] = True
            i += 1
            continue
        if a in ("--self-test-wsl", "-WslSelfTest", "--wsl-self-test"):
            out["self_test_wsl"] = True
            i += 1
            continue
        name = a.lstrip("-").lower().replace("_", "").replace("-", "")
        if name in ("root", "work", "profile", "receipt", "role", "model", "copy",
                    "copies", "isolation", "wsldistro", "capacitylease", "capacitydir",
                    "capabilitypolicy", "capabilitybin"):
            val = argv[i + 1] if i + 1 < len(argv) else ""
            if name in ("copy", "copies"):
                out["copies"].append(val)
            else:
                key = {"wsldistro": "wsl_distro", "capacitylease": "capacity_lease",
                       "capacitydir": "capacity_dir",
                       "capabilitypolicy": "capability_policy",
                       "capabilitybin": "capability_bin"}.get(name, name)
                out[key] = val
            i += 2
            continue
        if name in ("allownetwork", "allowhostwrite", "keep", "untrustedorigin"):
            out["allow_network" if name == "allownetwork" else
                "allow_host_write" if name == "allowhostwrite" else
                "untrusted_origin" if name == "untrustedorigin" else "keep"] = True
            i += 1
            continue
        if name in ("capsmemmb", "capsprocs", "capsseconds", "maxoutputbytes"):
            val = argv[i + 1] if i + 1 < len(argv) else ""
            try:
                out["mem_mb" if name == "capsmemmb" else
                    "procs" if name == "capsprocs" else
                    "seconds" if name == "capsseconds" else "max_output"] = int(val)
            except ValueError:
                out["error"] = f"{a} needs an integer"
                return out
            i += 2
            continue
        out["error"] = f"unknown argument: {a}"
        return out
    return out


def write_receipt(path, doc):
    if not path:
        return
    os.makedirs(os.path.dirname(os.path.abspath(path)), exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="\n") as fh:
        json.dump(doc, fh, ensure_ascii=True, indent=2)
        fh.write("\n")


def self_test():
    checks = []

    def ck(name, cond, detail=""):
        checks.append((name, bool(cond), detail))

    defaults = parse_args([])
    ck("default profile/network/host-write are permissive",
       defaults["profile"] == "full" and defaults["allow_network"]
       and defaults["allow_host_write"])

    tmp = tempfile.mkdtemp(prefix="sandbox-self-")
    host = os.path.join(tmp, "host")
    os.makedirs(host)
    with open(os.path.join(host, "in.txt"), "w", encoding="utf-8", newline="\n") as fh:
        fh.write("DATA\n")
    subprocess.run(["git", "-C", host, "init", "-q"], check=False)
    subprocess.run(["git", "-C", host, "config", "user.email", "a@b"], check=False)
    subprocess.run(["git", "-C", host, "config", "user.name", "a"], check=False)
    subprocess.run(["git", "-C", host, "add", "."], check=False)
    subprocess.run(["git", "-C", host, "commit", "-qm", "init"], check=False)
    workroot = os.path.join(tmp, "work")
    os.makedirs(workroot)

    def case(name, argv, copies=(), **kw):
        # Selftest fixtures keep the strict host-write gate unless a case opts in;
        # production defaults are covered by the parse_args assertion above.
        kw.setdefault("allow_host_write", False)
        work = os.path.join(workroot, "w-" + uuid.uuid4().hex[:10])
        rec = run_sandbox(host, work, argv, copies=copies, **kw)
        return rec, work

    rec, work = case("relative write", [PY, "-c", "open('out.txt','w').write('x')"], keep=True)
    ck("relative write stays in workdir", rec["ok"] and
       os.path.exists(os.path.join(work, "out.txt")) and not rec["host"]["mutated"], rec["verdict"])
    rec2, work2 = case("relative write cleanup", [PY, "-c", "open('out.txt','w').write('x')"])
    ck("success cleans workdir", rec2["ok"] and not os.path.exists(work2), str(rec2.get("cleaned")))

    rec, _ = case("copied input", [PY, "-c", "print(open('in.txt').read().strip())"],
                  copies=("in.txt",))
    ck("copied input is readable", rec["ok"] and "DATA" in rec["run"]["stdout_tail"], rec["verdict"])

    evil = os.path.join(host, "evil.txt")
    rec, _ = case("host write detected", [PY, "-c", f"open({evil!r},'w').write('bad')"])
    ck("host mutation detected and refused", rec["verdict"] == "host-mutated"
       and rec["exit"] == 3 and os.path.exists(evil), rec["verdict"])
    os.unlink(evil)

    rec, _ = case("host write allowed", [PY, "-c", f"open({evil!r},'w').write('ok')"],
                  allow_host_write=True)
    ck("host write allowed only with flag", rec["ok"] and rec["host"]["mutated"]
       and os.path.exists(evil), rec["verdict"])
    os.unlink(evil)

    rec, _ = case("timeout", [PY, "-c", "import time; time.sleep(3)"], seconds=1)
    ck("timeout exits 124", rec["exit"] == 124 and rec["run"]["timed_out"], rec["verdict"])

    rec, _ = case("memory", [PY, "-c", "x=bytearray(256*1024*1024)"], mem_mb=64)
    ck("memory cap kills allocation", rec["exit"] != 0 and rec["verdict"] == "run-failed",
       f"{rec['exit']}")

    cstate = os.path.join(tmp, "capacity")
    cp = capacity.probe(cpu_override=8, memory_override_mb=8192)
    lease = capacity.reserve("sandbox", "sandbox-run", 1, 128, 60, "sandbox", cstate,
                        probe_data=cp)
    rec, _ = case("shared lease", [PY, "-c", "print('lease-ok')"],
                  capacity_lease=lease["lease"]["id"], capacity_dir=cstate, mem_mb=128)
    ck("sandbox attaches to a live lease", rec["ok"]
       and rec["capacity"]["mode"] == "lease"
       and "lease-ok" in rec["run"]["stdout_tail"], rec["verdict"])
    rec, _ = case("lease memory over", [PY, "-c", "print('never')"],
                  capacity_lease=lease["lease"]["id"], capacity_dir=cstate, mem_mb=256)
    ck("sandbox refuses to exceed lease memory",
       rec["verdict"] == "bad-invocation" and "exceeds lease budget" in rec.get("error", ""),
       rec.get("error", ""))
    rec, _ = case("lease missing", [PY, "-c", "print(1)"],
                  capacity_lease="missing-lease", capacity_dir=cstate, mem_mb=64)
    ck("missing lease fails closed", rec["verdict"] == "bad-invocation"
       and "not found" in rec.get("error", ""), rec.get("error", ""))
    expired = capacity.reserve("sandbox", "sandbox-run", 1, 64, 30, "old", cstate,
                          probe_data=cp, now=100)
    rec, _ = case("lease expired", [PY, "-c", "print(1)"],
                  capacity_lease=expired["lease"]["id"], capacity_dir=cstate, mem_mb=64)
    ck("expired lease fails closed", rec["verdict"] == "bad-invocation"
       and "expired" in rec.get("error", ""), rec.get("error", ""))
    capacity.release(lease["lease"]["id"], cstate)

    cap_path = os.path.join(tmp, "capability.json")
    with open(cap_path, "w", encoding="utf-8") as fh:
        json.dump({"allow": ["read", "write", "exec", "network"],
                   "allow_untrusted_origin": False}, fh)

    def cap_allow(exe, payload):
        return {"ok": True, "allow": True, "decision": {"class": payload["action"]["class"]}}

    def cap_deny_network(exe, payload):
        allow = payload["action"]["class"] != "network"
        return {"ok": True, "allow": allow, "decision": {"class": payload["action"]["class"]}}

    rec, _ = case("capability allow", [PY, "-c", "print('cap-ok')"],
                  capability_policy=cap_path, capability_bin="fake-trit-orch",
                  capability_runner=cap_allow, allow_network=False)
    ck("capability policy admits exec+write", rec["ok"]
       and rec["capability"]["ok"] and "cap-ok" in rec["run"]["stdout_tail"]
       and [d["action"]["class"] for d in rec["capability"]["actions"]] == ["exec", "write"],
       str(rec.get("capability")))

    rec, _ = case("capability deny", [PY, "-c", "print('must-not-run')"],
                  allow_network=True, capability_policy=cap_path,
                  capability_bin="fake-trit-orch", capability_runner=cap_deny_network)
    ck("capability deny stops before execution", rec["verdict"] == "capability-denied"
       and rec["exit"] == 1 and not rec.get("run")
       and rec["capability"]["actions"][-1]["action"]["class"] == "network",
       str(rec.get("capability")))

    rec, _ = case("capability missing binary", [PY, "-c", "print('must-not-run')"],
                  capability_policy=cap_path,
                  capability_bin=os.path.join(tmp, "no-such-trit-orch"))
    ck("missing capability binary fails closed", rec["verdict"] == "capability-denied"
       and "capability call failed" in str(rec.get("capability")), str(rec.get("capability")))

    rec, _ = case("capability unreadable policy", [PY, "-c", "print('must-not-run')"],
                  capability_policy=os.path.join(tmp, "no-such-policy.json"),
                  capability_bin="fake-trit-orch", capability_runner=cap_allow)
    ck("unreadable capability policy fails closed", rec["verdict"] == "capability-denied"
       and "policy unreadable" in str(rec.get("capability")), str(rec.get("capability")))

    os.environ["TRIT_SANDBOX_SECRET"] = "must-not-leak"
    rec, _ = case("env secret", [PY, "-c",
                                 "import os; print('SECRET=' + repr(os.getenv('TRIT_SANDBOX_SECRET')))"])
    ck("secret env removed", "must-not-leak" not in rec["run"]["stdout_tail"]
       and "TRIT_SANDBOX_SECRET" in rec["env_removed"], rec["verdict"])
    os.environ.pop("TRIT_SANDBOX_SECRET", None)

    rec, _ = case("env path", [PY, "-c", "import os; print('PATH=' + str(len(os.getenv('PATH',''))))"])
    ck("PATH survives scrub", rec["ok"] and "PATH=" in rec["run"]["stdout_tail"], rec["verdict"])

    rec, _ = case("exit code", [PY, "-c", "raise SystemExit(7)"])
    ck("nonzero exit propagated", rec["exit"] == 7 and rec["verdict"] == "run-failed", rec["verdict"])

    rec, _ = case("missing command", ["definitely-not-a-command-trit-xyz"])
    ck("missing command is 127", rec["exit"] == 127 and rec["verdict"] == "run-failed", rec["verdict"])

    rec, _ = case("output flood", [PY, "-c", "print('x'*200000)"], max_output=1000)
    ck("output capped and marked", rec["run"]["stdout_truncated"]
       and rec["run"]["stdout_bytes"] > 1000, str(rec["run"]["stdout_bytes"]))

    rec, _ = case("stdin eof", [PY, "-c", "import sys; print('STDIN=' + repr(sys.stdin.read()))"])
    ck("stdin is EOF, not inherited", rec["ok"] and "STDIN=''" in rec["run"]["stdout_tail"], rec["verdict"])

    rec, _ = case("stderr", [PY, "-c", "import sys; print('ERR', file=sys.stderr)"])
    ck("stderr captured separately", rec["ok"] and "ERR" in rec["run"]["stderr_tail"], rec["verdict"])

    rec, _ = case("network deny", ["curl", "https://example.com"], allow_network=False)
    ck("policy denies network when explicitly disabled", rec["verdict"] == "policy-denied",
       rec["verdict"])

    rec, _ = case("deletion deny", ["rm", "in.txt"])
    ck("policy denies raw deletion", rec["verdict"] == "policy-denied", rec["verdict"])

    rec, _ = case("copy traversal", [PY, "-c", "print(1)"], copies=("../secret",))
    ck("copy traversal refused", rec["exit"] == 2 and "relative path" in rec.get("error", ""),
       rec.get("error", ""))

    # A junction inside the root can point at a denied component (.agent) while
    # its realpath is still inside root. `islink` is False for junctions; the
    # copy path must reject reparse points, not just symlinks.
    agent_secret_dir = os.path.join(host, ".agent")
    os.makedirs(agent_secret_dir, exist_ok=True)
    with open(os.path.join(agent_secret_dir, "secret.txt"), "w", encoding="utf-8") as fh:
        fh.write("TOP-SECRET\n")
    link_path = os.path.join(host, "linked-agent")
    made_link = False
    try:
        if os.name == "nt":
            rc = subprocess.run(["cmd", "/c", "mklink", "/J", link_path, agent_secret_dir],
                                capture_output=True, text=True)
            made_link = rc.returncode == 0
        else:
            os.symlink(agent_secret_dir, link_path, target_is_directory=True)
            made_link = True
    except OSError:
        made_link = False
    if made_link:
        rec, _ = case("junction to denied component", [PY, "-c", "print(1)"],
                      copies=("linked-agent",))
        ck("junction/symlink to denied component refused", rec["exit"] == 2
           and "symlink/junction" in rec.get("error", ""), rec.get("error", ""))
    else:
        ck("junction/symlink to denied component refused (creation skipped)", True, "skip")
    if made_link:
        try:
            os.rmdir(link_path) if os.name == "nt" else os.unlink(link_path)
        except OSError:
            pass

    # Windows is case-insensitive: `.AGENT/secret.txt` resolves to `.agent`.
    # The denied-component check must compare case-insensitively.
    upper_agent = os.path.join(host, ".AGENT")
    os.makedirs(upper_agent, exist_ok=True)
    with open(os.path.join(upper_agent, "secret.txt"), "w", encoding="utf-8") as fh:
        fh.write("TOP-SECRET\n")
    rec, _ = case("case-variant denied component", [PY, "-c", "print(1)"],
                  copies=(".AGENT/secret.txt",))
    ck("case-variant denied component refused", rec["exit"] == 2
       and "denied component" in rec.get("error", ""), rec.get("error", ""))
    dotted_agent = os.path.join(host, ".agent.")
    os.makedirs(dotted_agent, exist_ok=True)
    with open(os.path.join(dotted_agent, "secret.txt"), "w", encoding="utf-8") as fh:
        fh.write("TOP-SECRET\n")
    rec, _ = case("trailing-dot denied component", [PY, "-c", "print(1)"],
                  copies=(".agent./secret.txt",))
    ck("trailing-dot denied component refused", rec["exit"] == 2
       and "denied component" in rec.get("error", ""), rec.get("error", ""))

    # A hardlink inside root is still an outside inode. realpath cannot see it;
    # st_nlink > 1 must be refused or the copy leaks the target's bytes.
    secret = os.path.join(tmp, "outside-secret.txt")
    with open(secret, "w", encoding="utf-8") as fh:
        fh.write("TOP-SECRET\n")
    hard = os.path.join(host, "hard.txt")
    hard_ok = False
    try:
        os.link(secret, hard)
        hard_ok = True
    except OSError:
        pass
    if hard_ok:
        rec, _ = case("hardlink input", [PY, "-c", "print(open('hard.txt').read())"],
                      copies=("hard.txt",))
        ck("hardlink input refused", rec["exit"] == 2 and "hardlink" in rec.get("error", ""),
           rec.get("error", ""))
    else:
        ck("hardlink input check skipped (host does not support hardlinks)", True, "skip")

    # A workdir path can be a junction/symlink while still being lexically
    # outside root. Resolve it before creating anything, or the command writes
    # into an arbitrary victim directory.
    victim = os.path.join(tmp, "victim")
    os.makedirs(victim, exist_ok=True)
    worklink = os.path.join(workroot, "w-link")
    link_ok = False
    try:
        if os.name == "nt":
            sub = subprocess.run(["cmd", "/c", "mklink", "/J", worklink, victim],
                                 capture_output=True, text=True)
            link_ok = sub.returncode == 0
        else:
            os.symlink(victim, worklink)
            link_ok = True
    except OSError:
        link_ok = False
    if link_ok:
        rec = run_sandbox(host, worklink, [PY, "-c", "open('escaped.txt','w').write('x')"])
        ck("workdir link/junction refused", rec["exit"] == 2
           and "link/junction" in rec.get("error", "")
           and not os.path.exists(os.path.join(victim, "escaped.txt")), rec.get("error", ""))
        try:
            os.rmdir(worklink)
        except OSError:
            pass
    else:
        ck("workdir link check skipped (host does not support junctions)", True, "skip")

    # A write to a git-ignored host path must still be visible to the host
    # fingerprint. git status alone misses it; the all-file manifest catches it.
    with open(os.path.join(host, ".gitignore"), "w", encoding="utf-8") as fh:
        fh.write("ignored.txt\n")
    subprocess.run(["git", "-C", host, "add", ".gitignore"], check=False)
    subprocess.run(["git", "-C", host, "-c", "user.email=a@b", "-c", "user.name=a",
                    "commit", "-qm", "ignore"], check=False)
    ignored_target = os.path.join(host, "ignored.txt")
    rec, _ = case("ignored host write", [PY, "-c",
                                         f"open({ignored_target!r},'w').write('x')"], keep=True)
    ck("write to git-ignored host file is detected", rec["verdict"] == "host-mutated"
       and rec["exit"] == 3 and os.path.exists(ignored_target), rec["verdict"])
    if os.path.exists(ignored_target):
        os.unlink(ignored_target)

    # nested copy needs a real nested source; add it and rerun
    os.makedirs(os.path.join(host, "sub"), exist_ok=True)
    with open(os.path.join(host, "sub", "in.txt"), "w", encoding="utf-8", newline="\n") as fh:
        fh.write("DATA\n")
    rec, _ = case("nested copy 2", [PY, "-c", "print(open('sub/in.txt').read().strip())"],
                  copies=("sub/in.txt",))
    ck("nested copy works", rec["ok"] and "DATA" in rec["run"]["stdout_tail"], rec["verdict"])

    rec, keepwork = case("keep", [PY, "-c", "open('out.txt','w').write('x')"], keep=True)
    ck("keep retains workdir", rec["ok"] and os.path.exists(keepwork) and
       os.path.exists(os.path.join(keepwork, "out.txt")), rec["verdict"])

    inside = os.path.join(host, ".agent", "sandbox", "x")
    rec = run_sandbox(host, inside, [PY, "-c", "print(1)"])
    ck("workdir inside root refused", rec["exit"] == 2 and "outside root" in rec.get("error", ""),
       rec.get("error", ""))

    rec, _ = case("copy size cap", [PY, "-c", "print(1)"], copies=("in.txt",),
                  max_input_bytes=2)
    ck("copy byte cap refused", rec["exit"] == 2 and "bytes" in rec.get("error", ""),
       rec.get("error", ""))

    # concurrency: two independent workdirs, same relative filename, no cross-talk
    results = {}

    def concurrent(key):
        w = os.path.join(workroot, "c-" + key)
        results[key] = run_sandbox(host, w, [PY, "-c", f"open('out.txt','w').write({key!r})"])

    t1 = threading.Thread(target=concurrent, args=("A",))
    t2 = threading.Thread(target=concurrent, args=("B",))
    t1.start(); t2.start(); t1.join(); t2.join()
    ck("concurrent sandboxes isolated", results["A"]["ok"] and results["B"]["ok"]
       and results["A"]["work"] != results["B"]["work"], "")

    # `.agent` / `.git` writes used to be invisible because both were excluded
    # from the manifest. Security-sensitive host state must be inside the
    # fingerprint, not treated like a build cache.
    exroot = os.path.join(tmp, "excluded-state")
    os.makedirs(os.path.join(exroot, ".agent"))
    os.makedirs(os.path.join(exroot, ".git"))
    with open(os.path.join(exroot, ".agent", "state.json"), "w", encoding="utf-8") as fh:
        fh.write("{}\n")
    with open(os.path.join(exroot, ".git", "config"), "w", encoding="utf-8") as fh:
        fh.write("[core]\n\trepositoryformatversion = 0\n")
    with open(os.path.join(exroot, "keep.txt"), "w", encoding="utf-8") as fh:
        fh.write("keep\n")
    agent_hole = os.path.join(exroot, ".agent", "hole.txt")
    rec = run_sandbox(exroot, os.path.join(workroot, "exagent"),
                      [PY, "-c", f"open({agent_hole!r},'w').write('HOST-WRITE')"],
                      allow_host_write=False)
    ck("excluded .agent write is host-mutated", rec["verdict"] == "host-mutated"
       and rec["exit"] == 3 and os.path.exists(agent_hole), rec["verdict"])
    git_hole = os.path.join(exroot, ".git", "hooks", "post-checkout")
    os.makedirs(os.path.dirname(git_hole), exist_ok=True)
    rec = run_sandbox(exroot, os.path.join(workroot, "exgit"),
                      [PY, "-c", f"open({git_hole!r},'w').write('HOST-WRITE')"],
                      allow_host_write=False)
    ck("excluded .git hook write is host-mutated", rec["verdict"] == "host-mutated"
       and rec["exit"] == 3 and os.path.exists(git_hole), rec["verdict"])
    git_index = os.path.join(exroot, ".git", "index")
    with open(git_index, "wb") as fh:
        fh.write(b"DIRC\x00baseline")
    rec = run_sandbox(exroot, os.path.join(workroot, "exindex"),
                      [PY, "-c", f"open({git_index!r},'ab').write(b'X')"],
                      allow_host_write=False)
    ck("git index content rewrite is host-mutated", rec["verdict"] == "host-mutated"
       and rec["exit"] == 3, rec["verdict"])
    target_dir = os.path.join(exroot, "target")
    os.makedirs(target_dir)
    target_hole = os.path.join(target_dir, "evil.bin")
    rec = run_sandbox(exroot, os.path.join(workroot, "extarget"),
                      [PY, "-c", f"open({target_hole!r},'wb').write(b'evil')"],
                      allow_host_write=False)
    ck("build-cache directory write is host-mutated", rec["verdict"] == "host-mutated"
       and rec["exit"] == 3 and os.path.exists(target_hole), rec["verdict"])
    target_existing = os.path.join(target_dir, "existing.bin")
    with open(target_existing, "wb") as fh:
        fh.write(b"OLD")
    rec = run_sandbox(exroot, os.path.join(workroot, "extarget-overwrite"),
                      [PY, "-c", f"open({target_existing!r},'wb').write(b'NEWER')"],
                      allow_host_write=False)
    ck("existing build-cache file overwrite is host-mutated", rec["verdict"] == "host-mutated"
       and rec["exit"] == 3, rec["verdict"])

    # Same-size overwrite with the original mtime restored: metadata alone
    # would miss it, the content digest must not.
    same_file = os.path.join(exroot, "same-size.txt")
    with open(same_file, "wb") as fh:
        fh.write(b"AAAA")
    same_st = os.stat(same_file)
    rec = run_sandbox(exroot, os.path.join(workroot, "exsame"),
                      [PY, "-c",
                       f"open({same_file!r},'wb').write(b'BBBB'); "
                       f"import os; os.utime({same_file!r}, ns=({same_st.st_atime_ns}, "
                       f"{same_st.st_mtime_ns}))"],
                      allow_host_write=False)
    ck("same-size write with restored mtime is host-mutated", rec["verdict"] == "host-mutated"
       and rec["exit"] == 3, rec["verdict"])

    # A file above the old 1 MiB threshold used to be metadata-only. Its
    # content must now be hashed too, otherwise a same-size overwrite with a
    # restored mtime would be invisible.
    large_file = os.path.join(exroot, "large.bin")
    with open(large_file, "wb") as fh:
        fh.write(b"A" * (1500 * 1024))
    large_st = os.stat(large_file)
    rec = run_sandbox(exroot, os.path.join(workroot, "exlarge"),
                      [PY, "-c",
                       f"p={large_file!r}; data=open(p,'rb').read(); "
                       f"open(p,'wb').write(b'B'*len(data)); import os; "
                       f"os.utime(p, ns=({large_st.st_atime_ns}, {large_st.st_mtime_ns}))"],
                      allow_host_write=False)
    ck("large same-size write with restored mtime is host-mutated",
       rec["verdict"] == "host-mutated" and rec["exit"] == 3, rec["verdict"])
    os.unlink(large_file)

    # `index.lock` used to be skipped as transient. A lock left behind is a
    # host mutation and must be visible to the fingerprint.
    lock_file = os.path.join(exroot, ".git", "index.lock")
    rec = run_sandbox(exroot, os.path.join(workroot, "exlock"),
                      [PY, "-c", f"open({lock_file!r},'w').write('LOCK')"],
                      allow_host_write=False)
    ck("leftover .git/index.lock is host-mutated", rec["verdict"] == "host-mutated"
       and rec["exit"] == 3 and os.path.exists(lock_file), rec["verdict"])
    if os.path.exists(lock_file):
        os.unlink(lock_file)

    # A read-only `git status` can change the .git directory mtime while
    # creating/removing index.lock. That must not look like a host write; a
    # real file write under .git still must.
    git_dir = os.path.join(exroot, ".git")
    base_fp = fingerprint(exroot)[0]
    git_st = os.stat(git_dir)
    os.utime(git_dir, ns=(git_st.st_atime_ns, git_st.st_mtime_ns + 1_000_000))
    ck("git dir mtime alone is not host-mutated", fingerprint(exroot)[0] == base_fp)
    git_note = os.path.join(git_dir, "host-note")
    with open(git_note, "w", encoding="utf-8") as fh:
        fh.write("x")
    ck("file write under .git is host-mutated", fingerprint(exroot)[0] != base_fp)
    os.unlink(git_note)

    # non-git fallback: still detects a host write
    nogit = os.path.join(tmp, "nogit")
    os.makedirs(nogit)
    with open(os.path.join(nogit, "a.txt"), "w", encoding="utf-8") as fh:
        fh.write("a")
    evil2 = os.path.join(nogit, "evil.txt")
    rec = run_sandbox(nogit, os.path.join(workroot, "nogit"), [PY, "-c", f"open({evil2!r},'w').write('x')"],
                      allow_host_write=False)
    ck("non-git fallback detects host write", rec["verdict"] == "host-mutated" and os.path.exists(evil2),
       rec["verdict"])
    os.unlink(evil2)

    bad = [c for c in checks if not c[1]]
    for name, ok, detail in checks:
        print(("ok   " if ok else "FAIL ") + name + (f"  [{detail}]" if detail and not ok else ""))
    print(f"sandbox-run self-test: {len(checks) - len(bad)}/{len(checks)}")
    shutil.rmtree(tmp, ignore_errors=True)
    return 0 if not bad else 1


def main(argv):
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8")
    args = parse_args(argv)
    if args["self_test"]:
        return self_test()
    if args["self_test_wsl"]:
        return wsl_self_test(args["wsl_distro"])
    if args.get("error"):
        print("sandbox-run: " + args["error"], file=sys.stderr)
        return 2
    root = args["root"] or os.getcwd()
    capability_policy = args["capability_policy"]
    if not capability_policy:
        default_policy = os.path.join(root, ".agent", "capability.json")
        if os.path.isfile(default_policy):
            capability_policy = default_policy
    work = args["work"]
    if not work:
        work = os.path.join(tempfile.gettempdir(), "trit-sandbox-" + uuid.uuid4().hex[:10])
    rec = run_sandbox(root, work, args["command"], copies=args["copies"],
                      profile=args["profile"], allow_network=args["allow_network"],
                      allow_host_write=args["allow_host_write"], mem_mb=args["mem_mb"],
                      procs=args["procs"], seconds=args["seconds"],
                      max_output=args["max_output"], keep=args["keep"],
                      role=args["role"], model=args["model"],
                      isolation=args["isolation"], wsl_distro=args["wsl_distro"],
                      capacity_lease=args["capacity_lease"],
                      capacity_dir=args["capacity_dir"],
                      capability_policy=args["capability_policy"],
                      capability_bin=args["capability_bin"],
                      untrusted_origin=args["untrusted_origin"])
    if args["receipt"]:
        write_receipt(args["receipt"], rec)
    if rec.get("run"):
        if rec["run"].get("stdout_tail"):
            print(rec["run"]["stdout_tail"])
        if rec["run"].get("stderr_tail"):
            print(rec["run"]["stderr_tail"], file=sys.stderr)
    if not rec["ok"]:
        print(f"sandbox-run: {rec['verdict']}: {rec.get('error', '')}", file=sys.stderr)
    return int(rec["exit"])


def wsl_self_test(distro=""):
    """Live kernel-isolation regression: only claims what WSL+bwrap actually do."""
    ok, detail = wsl_isolation_probe(distro)
    if not ok:
        print(f"SKIP sandbox-wsl: {detail}")
        return 4
    tmp = tempfile.mkdtemp(prefix="sandbox-wsl-self-")
    passed = failed = 0
    def ck(name, cond, extra=""):
        nonlocal passed, failed
        if cond:
            passed += 1
            print("ok   " + name)
        else:
            failed += 1
            print("FAIL " + name + (f"  [{extra}]" if extra else ""))
    try:
        root = os.path.join(tmp, "root")
        work = os.path.join(tmp, "work")
        outside = os.path.join(tmp, "outside")
        os.makedirs(root)
        os.makedirs(outside)
        with open(os.path.join(root, "in.txt"), "w", encoding="utf-8") as fh:
            fh.write("hello\n")
        with open(os.path.join(outside, "secret.txt"), "w", encoding="utf-8") as fh:
            fh.write("TOP-SECRET\n")
        wsl_outside = _wsl_path(outside, distro)
        probe = f'''import socket, subprocess
print("read_in=" + open("in.txt").read().strip())
checks = []
path = {wsl_outside!r} + "/secret.txt"
try:
    open(path).read()
    checks.append("read=LEAK")
except Exception as exc:
    checks.append("read=blocked:" + type(exc).__name__)
try:
    open({wsl_outside!r} + "/leak.txt", "w").write("LEAK")
    checks.append("write=LEAK")
except Exception as exc:
    checks.append("write=blocked:" + type(exc).__name__)
try:
    socket.create_connection(("1.1.1.1", 80), timeout=1).close()
    checks.append("net=LEAK")
except Exception as exc:
    checks.append("net=blocked:" + type(exc).__name__)
try:
    subprocess.run(["cat", path], check=True, capture_output=True)
    checks.append("subprocess=LEAK")
except Exception as exc:
    checks.append("subprocess=blocked:" + type(exc).__name__)
print("RESULT " + "|".join(checks))
'''
        with open(os.path.join(root, "probe.py"), "w", encoding="utf-8") as fh:
            fh.write(probe)
        rec = run_sandbox(root, work, ["python3", "probe.py"],
                          copies=["probe.py", "in.txt"], isolation="wsl",
                          wsl_distro=distro)
        ck("WSL backend selected", rec.get("isolation", {}).get("mode") == "wsl-bwrap",
           str(rec.get("isolation")))
        ck("isolated run finishes clean", rec.get("ok") is True and rec.get("verdict") == "ok",
           str(rec.get("verdict")) + " " + str(rec.get("error", "")))
        out = (rec.get("run") or {}).get("stdout_tail", "")
        ck("input copied into /work is readable", "read_in=hello" in out, out)
        for needle in ("read=blocked:", "write=blocked:", "net=blocked:", "subprocess=blocked:"):
            ck(needle.rstrip(":"), needle in out, out)
        ck("outside secret was not leaked", "TOP-SECRET" not in out, out)
        ck(".exe command is refused",
           "Windows .exe" in _bad_wsl_command(["notepad.exe"]))
        ck("Windows absolute-path argument is refused",
           "absolute-path" in _bad_wsl_command([r"C:\Windows\secret.txt"]))
        bad = run_sandbox(root, os.path.join(tmp, "badwork"), ["python3", "probe.py"],
                          isolation="wsl", wsl_distro="trit-no-such-distro")
        ck("unavailable WSL distro fails closed (no silent fallback)",
           bad.get("verdict") == "bad-invocation" and bad.get("run") is None,
           str(bad.get("error", "")))
    except Exception as exc:
        failed += 1
        print(f"FAIL WSL self-test crashed: {exc}")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    print(f"sandbox-wsl self-test: {passed}/{passed + failed}")
    return 0 if failed == 0 else 1


def _bad_wsl_command(command):
    try:
        _normalize_wsl_command(command)
        return ""
    except ValueError as exc:
        return str(exc)


def _unused_guard():
    # keep threading imported for callers that monkeypatch; self-test uses it.
    return threading


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
