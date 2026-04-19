"""Run the upstream Node.js test corpus (nodejs/node) under Wasmer's edgejs package."""

from __future__ import annotations

import concurrent.futures
import os
import re
import subprocess
import threading
from pathlib import Path

from python_upstream import append_log

# Official Node.js sources — *not* wasmerio/node-test (that fork is tuned for edgejs and is far too green).
DEFAULT_NODE_UPSTREAM_REPO = "https://github.com/nodejs/node.git"
# v24.13.1 (LTS); closest stable tag to edgejs's 24.13.x line. Pin a tag or full SHA.
DEFAULT_NODE_UPSTREAM_REF = "v24.13.1"

# From edgejs `src/node_version.h` (line-up with the Node major line you are testing).
DEFAULT_EDGEJS_NODE_VERSION = "24.13.2"

DEFAULT_EDGEJS_PACKAGE = "wasmer/edgejs"

# Top-level directories under ``test/`` we skip (native addons, WPT, pummel, etc.).
SKIP_TOP_LEVEL_TEST_DIRS = frozenset(
    {
        "cctest",
        "benchmark",
        "addons",
        "doctool",
        "embedding",
        "overlapped-checker",
        "wasi",
        "v8-updates",
        "code-cache",
        "internet",
        "tick-processor",
        "pummel",
        "wpt",
        "system-ca",
    }
)

DONE_RE = re.compile(r"\]\s*:\s*Done\b")
SKIP_LINE_RE = re.compile(r"\bSKIP\b|#\s*SKIP\b", re.IGNORECASE)


def _slugify_upstream_ref(value: str) -> str:
    return "".join(ch if ch.isalnum() or ch in "._-" else "-" for ch in value.strip()) or "pinned"


def ensure_node_upstream_checkout(work_dir: Path, ref: str) -> Path:
    """Shallow-fetch a single ref (tag, branch, or commit) into .work/node-upstream/<slug>."""
    slug = _slugify_upstream_ref(ref)
    cache_root = work_dir / "node-upstream"
    checkout = cache_root / slug
    cache_root.mkdir(parents=True, exist_ok=True)
    if not (checkout / ".git").exists():
        checkout.mkdir(parents=True, exist_ok=True)
        subprocess.run(["git", "init"], cwd=checkout, check=True)
        subprocess.run(["git", "remote", "add", "origin", DEFAULT_NODE_UPSTREAM_REPO], cwd=checkout, check=True)
    subprocess.run(["git", "fetch", "--depth", "1", "origin", ref], cwd=checkout, check=True)
    subprocess.run(["git", "checkout", "-f", "-B", "compat-tests-node-upstream", "FETCH_HEAD"], cwd=checkout, check=True)
    return checkout


def node_repo_test_dir(repo_root: Path) -> Path:
    return repo_root / "test"


def write_wasmer_node_wrapper(
    *,
    path: Path,
    wasmer_bin: Path,
    mount_root: Path,
    edgejs_package: str,
) -> None:
    mount = str(mount_root.resolve())
    wb = str(wasmer_bin.resolve())
    text = f"""#!/usr/bin/env bash
set -euo pipefail
exec "{wb}" run -q --experimental-napi --net \\
  --volume "{mount}:{mount}" \\
  "{edgejs_package}" -- "$@"
"""
    path.write_text(text)
    path.chmod(path.stat().st_mode | 0o111)


_SKIP_PATH_PARTS = frozenset({"common", "fixtures", "tmp", "testpy"})
_SQLITE_ROOT_JUNK = frozenset({"next-db.js", "worker.js"})
_JS_SUFFIXES = frozenset({".js", ".mjs", ".cjs"})


def _effective_skip_top_dirs(extra_exclude_suites: frozenset[str] | None) -> set[str]:
    skip = set(SKIP_TOP_LEVEL_TEST_DIRS)
    if extra_exclude_suites:
        skip |= set(extra_exclude_suites)
    for part in os.environ.get("NODE_TEST_EXCLUDE_SUITES", "").split(","):
        p = part.strip()
        if p:
            skip.add(p)
    return skip


def collect_test_files(
    test_dir: Path,
    *,
    only_suites: frozenset[str] | None = None,
    extra_exclude_suites: frozenset[str] | None = None,
    max_tests: int | None = None,
) -> list[str]:
    """Collect JS paths under the single Node ``test/`` tree (relative to ``test/``).

    Walk ``test/**/*.js|mjs|cjs`` in one pass: skip whole top-level dirs such as ``wpt/``,
    ``addons/``, ``pummel/``, …; skip anything under ``common/``, ``fixtures/``, ``tmp/``,
    ``testpy/``; skip ``sqlite/{next-db,worker}.js`` at suite root.
    """
    skip_top = _effective_skip_top_dirs(extra_exclude_suites)
    out: list[str] = []
    for path in test_dir.rglob("*"):
        if not path.is_file():
            continue
        if path.suffix not in _JS_SUFFIXES or path.name.startswith("."):
            continue
        try:
            rel = path.relative_to(test_dir)
        except ValueError:
            continue
        parts = rel.parts
        if len(parts) < 2:
            continue
        top = parts[0]
        if top in skip_top:
            continue
        if only_suites is not None and top not in only_suites:
            continue
        if set(parts) & _SKIP_PATH_PARTS:
            continue
        if "node_modules" in parts:
            continue
        if top == "sqlite" and len(parts) == 2 and parts[1] in _SQLITE_ROOT_JUNK:
            continue
        out.append(rel.as_posix())
    ordered = sorted(set(out))
    if max_tests is not None:
        return ordered[: max(max_tests, 0)]
    return ordered


def count_tests_by_suite(rel_paths: list[str]) -> dict[str, int]:
    counts: dict[str, int] = {}
    for rel in rel_paths:
        suite = rel.split("/", 1)[0]
        counts[suite] = counts.get(suite, 0) + 1
    return dict(sorted(counts.items()))


def parse_node_single_file_status(*, stdout: str, stderr: str, exit_code: int, timed_out: bool) -> str:
    if timed_out:
        return "TIMEOUT"
    text = (stdout or "") + "\n" + (stderr or "")
    if SKIP_LINE_RE.search(text):
        return "SKIP"
    if exit_code == 0 and ("All tests passed" in text or "All tests succeeded" in text):
        return "PASS"
    for line in text.splitlines():
        if DONE_RE.search(line):
            if re.search(r"-\s*0\b", line) and re.search(r"\+\s*[1-9]", line):
                return "PASS" if exit_code == 0 else "FAIL"
            if re.search(r"-\s*[1-9]", line):
                return "FAIL"
    if exit_code == 0:
        return "PASS"
    return "FAIL"


def run_node_debug(
    *,
    repo_root: Path,
    wrapper: Path,
    rel_test: str,
    timeout: int | None,
    test_py_timeout: int,
) -> subprocess.CompletedProcess[str]:
    test_dir = node_repo_test_dir(repo_root)
    test_py = repo_root / "tools" / "test.py"
    cmd = [
        "python3",
        str(test_py),
        "--test-root",
        str(test_dir),
        "--shell",
        str(wrapper),
        "--timeout",
        str(test_py_timeout),
        "--progress",
        "mono",
        rel_test,
    ]
    print("+", " ".join(cmd), flush=True)
    return subprocess.run(cmd, cwd=repo_root, text=True, capture_output=True, timeout=timeout)


def run_node_upstream(
    *,
    repo_root: Path,
    wrapper: Path,
    timeout: int,
    test_py_timeout: int,
    log_path: Path | None,
    only_suites: frozenset[str] | None,
    extra_exclude_suites: frozenset[str] | None,
    jobs_limit: int | None,
    max_tests: int | None,
) -> dict[str, str]:
    test_dir = node_repo_test_dir(repo_root)
    test_py = repo_root / "tools" / "test.py"
    tests = collect_test_files(
        test_dir,
        only_suites=only_suites,
        extra_exclude_suites=extra_exclude_suites,
        max_tests=max_tests,
    )
    if not tests:
        raise RuntimeError("No upstream Node test files matched filters")

    workers = (getattr(os, "process_cpu_count", os.cpu_count)() or 1) + 2
    if jobs_limit is not None:
        workers = min(workers, max(jobs_limit, 1))
    workers = min(workers, len(tests))

    status: dict[str, str] = {}
    log_lock = threading.Lock() if log_path is not None else None
    completed = 0
    total = len(tests)

    def run_one(rel: str) -> tuple[str, str, str, str]:
        timed_out = False
        try:
            proc = subprocess.run(
                [
                    "python3",
                    str(test_py),
                    "--test-root",
                    str(test_dir),
                    "--shell",
                    str(wrapper),
                    "--timeout",
                    str(test_py_timeout),
                    "--progress",
                    "mono",
                    rel,
                ],
                cwd=repo_root,
                text=True,
                capture_output=True,
                timeout=max(timeout, 2),
            )
            out, err, code = proc.stdout or "", proc.stderr or "", proc.returncode
        except subprocess.TimeoutExpired as exc:
            out = (exc.stdout.decode() if isinstance(exc.stdout, bytes) else exc.stdout) or ""
            err = (exc.stderr.decode() if isinstance(exc.stderr, bytes) else exc.stderr) or ""
            code = -1
            timed_out = True
        label = parse_node_single_file_status(stdout=out, stderr=err, exit_code=code, timed_out=timed_out)
        return rel, label, out, err

    print(f"Running {total} upstream Node test files with {workers} workers (outer timeout {timeout}s)...", flush=True)
    with concurrent.futures.ThreadPoolExecutor(max_workers=workers) as pool:
        futures = {pool.submit(run_one, rel): rel for rel in tests}
        for future in concurrent.futures.as_completed(futures):
            rel, label, out, err = future.result()
            append_log(log_path, log_lock, rel, out, err)
            status[rel] = label
            completed += 1
            print(f"[{completed}/{total}] {rel} {label}", flush=True)

    return dict(sorted(status.items()))
