#!/usr/bin/env python3
"""run-caps: run one command under kernel-level resource caps. stdlib only (ctypes).

    run-caps --mem-mb N --procs N --seconds S [--lease ID --capacity-dir DIR] -- <cmd> [args...]

At least one cap must be requested: running with no cap at all would silently differ
from the promise this tool makes. Caps that are not given simply do not apply.

--lease attaches the run to an existing capacity lease. The lease must still be alive
(not past expires_at); its memory budget becomes the ceiling, and a larger --mem-mb is
refused instead of silently overcommitting the host.

Caps, stated one by one:
  --mem-mb   per-process committed-memory ceiling. On Windows this is the job object's
             PROCESS_MEMORY limit: the KERNEL fails allocations past the cap in every
             process of the job (the process sees a hard allocation failure, e.g. a
             Python MemoryError, and dies on its own). On POSIX it is RLIMIT_AS, which
             each child inherits for itself. Per process, NOT a sum over the tree.
  --procs    active-process ceiling for the whole job. Kernel-enforced on Windows: the
             (N+1)-th CreateProcess fails. POSIX has NO equivalent here, on purpose:
             RLIMIT_NPROC counts processes per USER, so lowering it in a child would
             throttle every process of the same account. Stated honestly, not faked.
  --seconds  parent-side watchdog (not a kernel timer): on timeout the whole job --
             parent, children, grandchildren -- is killed and run-caps exits 124.
             Otherwise the child's exit code is passed through unchanged (timeout=124
             is the only mapping).

KILL_ON_JOB_CLOSE: if this parent itself dies, the system closes its job handle and
every process in the job dies with it (POSIX: the child gets its own session; a
grandchild that escapes the process group by setsid() is not reaped -- stated honestly).

Honest strength statement -- this is a resource cap, NOT a security boundary:
  * it does not isolate the filesystem or the network;
  * it does not protect against a deliberately escaping process;
  * it complements policy, and the division of labour is fixed: policy is the pre-run
    command classifier that stops misuse (dangerous commands, out-of-root writes) BEFORE
    anything starts; run-caps bounds the damage if a permitted command runs away. One
    gates, the other contains. Neither pretends to do the other's job.

agent-verify integration: with --caps, every run-segment command is wrapped as

    run_caps.py --mem-mb .. --procs .. --seconds .. -- python _run.py -Root .. -Command ..

so the caps cover the runner and everything it spawns (job membership is inherited;
the shell-picking lesson of L44/T10 stays in exactly one place, _run.py).

Exit codes: the child's exit code; 124 timeout; 127 cannot start; 2 bad invocation or
cap setup failed (fail-closed: nothing runs uncapped because setup broke).
"""

import os
import subprocess
import sys

MB = 1024 * 1024

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import capacity_lease as cap  # noqa: E402  (same-directory tool, stdlib only)

if os.name == "nt":
    import ctypes
    from ctypes import wintypes

    class _IO_COUNTERS(ctypes.Structure):
        _fields_ = [(n, ctypes.c_ulonglong) for n in
                    ("ReadOperationCount", "WriteOperationCount", "OtherOperationCount",
                     "ReadTransferCount", "WriteTransferCount", "OtherTransferCount")]

    # Field order and types must match winbase.h exactly: SetInformationJobObject takes
    # the struct size, and a wrong size is rejected with ERROR_INVALID_PARAMETER -- the
    # failure is loud (exit 2), never a silent uncapped run.
    class _BASIC_LIMIT(ctypes.Structure):
        _fields_ = [
            ("PerProcessUserTimeLimit", ctypes.c_longlong),
            ("PerJobUserTimeLimit", ctypes.c_longlong),
            ("LimitFlags", wintypes.DWORD),
            ("MinimumWorkingSetSize", ctypes.c_size_t),
            ("MaximumWorkingSetSize", ctypes.c_size_t),
            ("ActiveProcessLimit", wintypes.DWORD),
            ("Affinity", ctypes.c_size_t),
            ("PriorityClass", wintypes.DWORD),
            ("IoInfo", _IO_COUNTERS),
        ]

    class _EXT_LIMIT(ctypes.Structure):
        _fields_ = [
            ("Basic", _BASIC_LIMIT),
            ("ProcessMemoryLimit", ctypes.c_size_t),
            ("JobMemoryLimit", ctypes.c_size_t),
            ("PeakProcessMemoryUsed", ctypes.c_size_t),
            ("PeakJobMemoryUsed", ctypes.c_size_t),
        ]

    class _STARTUPINFOW(ctypes.Structure):
        _fields_ = [
            ("cb", wintypes.DWORD), ("lpReserved", wintypes.LPWSTR),
            ("lpDesktop", wintypes.LPWSTR), ("lpTitle", wintypes.LPWSTR),
            ("dwX", wintypes.DWORD), ("dwY", wintypes.DWORD),
            ("dwXSize", wintypes.DWORD), ("dwYSize", wintypes.DWORD),
            ("dwXCountChars", wintypes.DWORD), ("dwYCountChars", wintypes.DWORD),
            ("dwFillAttribute", wintypes.DWORD), ("dwFlags", wintypes.DWORD),
            ("wShowWindow", wintypes.WORD), ("cbReserved2", wintypes.WORD),
            ("lpReserved2", ctypes.c_void_p),
            ("hStdInput", wintypes.HANDLE), ("hStdOutput", wintypes.HANDLE),
            ("hStdError", wintypes.HANDLE),
        ]

    class _PROCESS_INFORMATION(ctypes.Structure):
        _fields_ = [("hProcess", wintypes.HANDLE), ("hThread", wintypes.HANDLE),
                    ("dwProcessId", wintypes.DWORD), ("dwThreadId", wintypes.DWORD)]

    # Flag bits and the info-class number are pinned by experiment, not memory: both
    # were wrong on the first write and the two errors (87/24) pointed at different
    # layers. The sweep that settled them is in ERRORS.md L97.
    _JOB_LIMIT_ACTIVE_PROCESS = 0x00000008
    _JOB_LIMIT_PROCESS_MEMORY = 0x00000100
    _JOB_LIMIT_KILL_ON_JOB_CLOSE = 0x00002000
    _JOBOBJECT_EXTENDED_LIMIT_INFORMATION = 9
    _CREATE_SUSPENDED = 0x00000004
    _WAIT_OBJECT_0 = 0x00000000
    _WAIT_TIMEOUT = 0x00000102
    _WAIT_INFINITE = 0xFFFFFFFF


def _run_windows(cmd, mem_bytes, procs, seconds):
    """Create the child suspended, put it in the job, then let it run.

    The suspended-create + assign + resume dance is not decoration: a child that runs
    one instruction before joining the job has a window in which it can spawn an
    uncapped grandchild. Window closed before the first instruction.
    """
    k32 = ctypes.WinDLL("kernel32", use_last_error=True)
    k32.CreateJobObjectW.restype = wintypes.HANDLE
    k32.WaitForSingleObject.restype = wintypes.DWORD
    k32.ResumeThread.restype = wintypes.DWORD

    hjob = k32.CreateJobObjectW(None, None)
    if not hjob:
        print(f"run-caps: CreateJobObjectW failed (winerror {ctypes.get_last_error()})")
        return 2

    ext = _EXT_LIMIT()
    flags = _JOB_LIMIT_KILL_ON_JOB_CLOSE
    if mem_bytes:
        flags |= _JOB_LIMIT_PROCESS_MEMORY
        ext.ProcessMemoryLimit = mem_bytes
    if procs:
        flags |= _JOB_LIMIT_ACTIVE_PROCESS
        ext.Basic.ActiveProcessLimit = procs
    ext.Basic.LimitFlags = flags
    if not k32.SetInformationJobObject(hjob, _JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
                                       ctypes.byref(ext), ctypes.sizeof(ext)):
        print(f"run-caps: SetInformationJobObject failed "
              f"(winerror {ctypes.get_last_error()})")
        k32.CloseHandle(hjob)
        return 2

    si = _STARTUPINFOW()
    si.cb = ctypes.sizeof(si)
    pi = _PROCESS_INFORMATION()
    cmdline = ctypes.create_unicode_buffer(subprocess.list2cmdline(cmd))
    if not k32.CreateProcessW(None, cmdline, None, None, True, _CREATE_SUSPENDED,
                              None, None, ctypes.byref(si), ctypes.byref(pi)):
        print(f"run-caps: cannot start '{cmd[0]}' (winerror {ctypes.get_last_error()})")
        k32.CloseHandle(hjob)
        return 127

    if not k32.AssignProcessToJobObject(hjob, pi.hProcess):
        # Fail-closed: a child outside the job would run uncapped, so kill it now.
        k32.TerminateProcess(pi.hProcess, 2)
        k32.CloseHandle(pi.hThread)
        k32.CloseHandle(pi.hProcess)
        k32.CloseHandle(hjob)
        print("run-caps: AssignProcessToJobObject failed -- child killed, "
              "nothing was left running uncapped")
        return 2
    k32.ResumeThread(pi.hThread)
    k32.CloseHandle(pi.hThread)

    timeout_ms = int(seconds * 1000) if seconds else _WAIT_INFINITE
    wait = k32.WaitForSingleObject(pi.hProcess, timeout_ms)
    if wait == _WAIT_TIMEOUT:
        k32.TerminateJobObject(hjob, 124)
        print(f"run-caps: seconds={seconds} exceeded -- job killed (exit 124)")
        code = 124
    elif wait != _WAIT_OBJECT_0:
        print(f"run-caps: WaitForSingleObject returned {wait} -- reporting exit 2")
        code = 2
    else:
        exit_code = wintypes.DWORD()
        k32.GetExitCodeProcess(pi.hProcess, ctypes.byref(exit_code))
        code = exit_code.value
    k32.CloseHandle(pi.hProcess)
    k32.CloseHandle(hjob)
    return code


def _run_posix(cmd, mem_bytes, procs, seconds):
    """POSIX fallback: RLIMIT_AS per process + own session + process-group reaping.

    Enforced here: the memory cap (each child inherits its own RLIMIT_AS) and the
    timeout (SIGKILL to the process group). NOT enforced here: the process-count cap
    (see the module docstring -- RLIMIT_NPROC counts per user and would hurt the whole
    account), and grandchildren that escape the group with a second setsid().
    """
    import resource

    def _pre():
        if mem_bytes:
            resource.setrlimit(resource.RLIMIT_AS, (mem_bytes, mem_bytes))
        os.setsid()

    if procs:
        print("run-caps: --procs is NOT enforced on POSIX (RLIMIT_NPROC counts per user "
              "and would throttle the whole account) -- stated, not faked")
    try:
        p = subprocess.Popen(cmd, preexec_fn=_pre)
    except OSError as e:
        print(f"run-caps: cannot start '{cmd[0]}': {e}")
        return 127
    try:
        return p.wait(timeout=seconds if seconds else None)
    except subprocess.TimeoutExpired:
        try:
            # 9 is SIGKILL on POSIX. Do not `import signal` here: this repo's
            # tools/signal.py would shadow the stdlib module on sys.path[0].
            os.killpg(os.getpgid(p.pid), 9)
        except OSError:
            p.kill()
        p.wait()
        print(f"run-caps: seconds={seconds} exceeded -- process group killed (exit 124)")
        return 124


def parse_args(argv):
    """Returns (caps, cmd, error). Flags before '--', the command is everything after."""
    caps = {"mem_mb": None, "procs": None, "seconds": None,
            "lease": "", "capacity_dir": ""}
    i = 0
    while i < len(argv) and argv[i] != "--":
        raw = argv[i]
        if not raw.startswith("-"):
            # A bare token before '--' is almost certainly the command typed too early;
            # refusing beats silently capping nothing.
            return None, None, f"missing '--' before the command (first bare token: {raw})"
        name = raw.lstrip("-").lower()
        if name in ("help", "h"):
            print(__doc__.strip())
            sys.exit(0)
        if name in ("lease", "capacity-dir"):
            if i + 1 >= len(argv):
                return None, None, f"{raw} needs a value"
            caps["lease" if name == "lease" else "capacity_dir"] = argv[i + 1]
            i += 2
            continue
        if name not in ("mem-mb", "procs", "seconds"):
            return None, None, f"unknown flag {raw}"
        if i + 1 >= len(argv):
            return None, None, f"{raw} needs a value"
        try:
            caps[name.replace("-", "_")] = int(argv[i + 1])
        except ValueError:
            return None, None, f"{raw} must be an integer"
        i += 2
    if i >= len(argv):
        return None, None, "missing '--' before the command (caps and command must not blur)"
    cmd = argv[i + 1:]
    return caps, cmd, None


def main(argv):
    caps, cmd, err = parse_args(argv)
    if err:
        print(f"run-caps: {err}")
        return 2
    if not cmd:
        print("run-caps: no command after '--'")
        return 2
    lease = None
    if caps["lease"]:
        try:
            loaded = cap.load_lease(caps["lease"], caps["capacity_dir"])
        except Exception as exc:  # fail closed: a broken broker must not uncap the run
            print(f"run-caps: lease lookup failed -- {type(exc).__name__}: {exc}")
            return 2
        if not loaded.get("ok"):
            print(f"run-caps: lease {caps['lease']} not found or unreadable")
            return 2
        lease = loaded["lease"]
        deadline = int(lease.get("deadline_at") or lease["expires_at"])
        if cap.now_epoch() >= float(lease["expires_at"]):
            print(f"run-caps: lease {caps['lease']} already expired")
            return 2
        if cap.now_epoch() >= deadline:
            print(f"run-caps: lease {caps['lease']} reached its hard deadline")
            return 2
        lease_mem = int(lease.get("resources", {}).get("memory_mb", 0))
        if caps["mem_mb"] is None:
            caps["mem_mb"] = lease_mem
        elif caps["mem_mb"] > lease_mem:
            print(f"run-caps: --mem-mb {caps['mem_mb']} exceeds lease memory {lease_mem}")
            return 2
    if caps["mem_mb"] is not None and caps["mem_mb"] <= 0:
        print("run-caps: --mem-mb must be >= 1")
        return 2
    if caps["procs"] is not None and caps["procs"] <= 0:
        print("run-caps: --procs must be >= 1")
        return 2
    if caps["seconds"] is not None and caps["seconds"] < 0:
        print("run-caps: --seconds must be >= 0 (0 disables the watchdog)")
        return 2
    if all(caps[k] is None for k in ("mem_mb", "procs", "seconds")):
        print("run-caps: no cap requested -- refuse (give at least one of "
              "--mem-mb/--procs/--seconds/--lease; an uncapped run would be a silent lie)")
        return 2

    mem_bytes = caps["mem_mb"] * MB if caps["mem_mb"] else None
    if os.name == "nt":
        return _run_windows(cmd, mem_bytes, caps["procs"], caps["seconds"])
    return _run_posix(cmd, mem_bytes, caps["procs"], caps["seconds"])


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
