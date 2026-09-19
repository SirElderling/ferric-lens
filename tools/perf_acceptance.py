#!/usr/bin/env python3
"""Measure Ferric Lens release-acceptance fixtures without gating on wall time."""

from __future__ import annotations

import argparse
import hashlib
from html.parser import HTMLParser
import importlib.util
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import tempfile
import time
from typing import Any


SCHEMA_VERSION = 1
SAMPLE_INTERVAL_SECONDS = 0.01
STANDARD_TARGETS = {
    "cold_analyze_wall_seconds_max": 10.0,
    "cold_analyze_peak_process_tree_rss_bytes_max": 256 * 1024 * 1024,
    "warm_check_wall_seconds_max": 2.0,
    "warm_check_peak_process_tree_rss_bytes_max": 128 * 1024 * 1024,
    "html_bytes_max": 10 * 1024 * 1024,
}
MODES = {
    "standard": (1, 1),
    "source-10x": (10, 1),
    "history-10x": (1, 10),
}


class CountingHtmlParser(HTMLParser):
    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self.start_tags = 0

    def handle_starttag(
        self,
        tag: str,
        attrs: list[tuple[str, str | None]],
    ) -> None:
        del tag, attrs
        self.start_tags += 1


def _fixture_module():
    script = Path(__file__).with_name("perf_fixture.py")
    spec = importlib.util.spec_from_file_location("perf_fixture", script)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


def process_tree_rss_kib(snapshot: str, root_pid: int) -> int:
    """Return aggregate RSS for root_pid and descendants from a ps snapshot."""

    parents: dict[int, int] = {}
    rss: dict[int, int] = {}
    for raw_line in snapshot.splitlines():
        fields = raw_line.split()
        if len(fields) != 3:
            continue
        try:
            pid, ppid, rss_kib = (int(field) for field in fields)
        except ValueError:
            continue
        parents[pid] = ppid
        rss[pid] = rss_kib

    if root_pid not in rss:
        return 0

    descendants = {root_pid}
    changed = True
    while changed:
        changed = False
        for pid, parent in parents.items():
            if pid not in descendants and parent in descendants:
                descendants.add(pid)
                changed = True

    return sum(rss.get(pid, 0) for pid in descendants)


def _ps_snapshot() -> str:
    completed = subprocess.run(
        ["ps", "-axo", "pid=,ppid=,rss="],
        text=True,
        capture_output=True,
        check=False,
    )
    return completed.stdout if completed.returncode == 0 else ""


def directory_size(root: Path) -> int:
    if not root.exists():
        return 0
    total = 0
    for path in root.rglob("*"):
        try:
            if path.is_file():
                total += path.stat().st_size
        except FileNotFoundError:
            continue
    return total


def _wait4_measurement(
    process: subprocess.Popen[Any],
    *,
    sample_interval: float,
) -> tuple[int, float, int]:
    peak_rss_kib = 0
    child_cpu_seconds = 0.0

    if hasattr(os, "wait4"):
        while True:
            peak_rss_kib = max(
                peak_rss_kib,
                process_tree_rss_kib(_ps_snapshot(), process.pid),
            )
            waited_pid, status, usage = os.wait4(process.pid, os.WNOHANG)
            if waited_pid == process.pid:
                exit_code = os.waitstatus_to_exitcode(status)
                process.returncode = exit_code
                child_cpu_seconds = float(usage.ru_utime + usage.ru_stime)
                break
            time.sleep(sample_interval)
        return exit_code, child_cpu_seconds, peak_rss_kib

    before = os.times()
    while process.poll() is None:
        peak_rss_kib = max(
            peak_rss_kib,
            process_tree_rss_kib(_ps_snapshot(), process.pid),
        )
        time.sleep(sample_interval)
    after = os.times()
    child_cpu_seconds = max(
        0.0,
        (after.children_user + after.children_system)
        - (before.children_user + before.children_system),
    )
    return int(process.returncode or 0), child_cpu_seconds, peak_rss_kib


def measure_command(
    args: list[str],
    *,
    cwd: Path,
    stdout_path: Path,
    stderr_path: Path,
    sample_interval: float = SAMPLE_INTERVAL_SECONDS,
) -> dict[str, Any]:
    stdout_path.parent.mkdir(parents=True, exist_ok=True)
    stderr_path.parent.mkdir(parents=True, exist_ok=True)

    started = time.monotonic()
    with stdout_path.open("wb") as stdout, stderr_path.open("wb") as stderr:
        process = subprocess.Popen(
            args,
            cwd=cwd,
            stdout=stdout,
            stderr=stderr,
        )
        exit_code, child_cpu_seconds, peak_rss_kib = _wait4_measurement(
            process,
            sample_interval=sample_interval,
        )
    wall_seconds = time.monotonic() - started

    return {
        "command": args,
        "exit_code": exit_code,
        "wall_seconds": wall_seconds,
        "child_cpu_seconds": child_cpu_seconds,
        "peak_process_tree_rss_kib": peak_rss_kib,
        "rss_sample_interval_seconds": sample_interval,
    }


def _command_output(args: list[str]) -> str | None:
    completed = subprocess.run(
        args,
        text=True,
        capture_output=True,
        check=False,
    )
    if completed.returncode != 0:
        return None
    return completed.stdout.strip()


def _total_memory_bytes() -> int | None:
    meminfo = Path("/proc/meminfo")
    if meminfo.is_file():
        for line in meminfo.read_text(encoding="utf-8").splitlines():
            if line.startswith("MemTotal:"):
                fields = line.split()
                if len(fields) >= 2:
                    return int(fields[1]) * 1024

    if sys.platform == "darwin":
        value = _command_output(["sysctl", "-n", "hw.memsize"])
        if value and value.isdigit():
            return int(value)
    return None


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _environment(binary: Path) -> dict[str, Any]:
    return {
        "platform": platform.platform(),
        "machine": platform.machine(),
        "logical_cpus": os.cpu_count(),
        "total_memory_bytes": _total_memory_bytes(),
        "python": platform.python_version(),
        "git": _command_output(["git", "--version"]),
        "rustc": _command_output(["rustc", "--version"]),
        "cargo": _command_output(["cargo", "--version"]),
        "binary_sha256": _sha256(binary),
    }


def _parse_html(path: Path) -> dict[str, Any]:
    parser = CountingHtmlParser()
    started = time.monotonic()
    parser.feed(path.read_text(encoding="utf-8"))
    elapsed = time.monotonic() - started
    return {
        "python_html_parse_seconds": elapsed,
        "start_tag_count": parser.start_tags,
        "note": (
            "Portable parser cost only; browser DOM/open cost remains a separate "
            "release-acceptance observation."
        ),
    }


def _replace_first_line(path: Path, replacement: str) -> None:
    lines = path.read_text(encoding="utf-8").splitlines()
    if not lines:
        raise RuntimeError(f"cannot edit empty fixture file: {path}")
    lines[0] = replacement
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")


def _standard_target_observations(
    *,
    cold: dict[str, Any],
    warm: dict[str, Any],
    html_bytes: int,
) -> dict[str, Any]:
    cold_rss_bytes = cold["peak_process_tree_rss_kib"] * 1024
    warm_rss_bytes = warm["peak_process_tree_rss_kib"] * 1024
    return {
        "targets": STANDARD_TARGETS,
        "observed": {
            "cold_analyze_wall_seconds": cold["wall_seconds"],
            "cold_analyze_peak_process_tree_rss_bytes": cold_rss_bytes,
            "warm_check_wall_seconds": warm["wall_seconds"],
            "warm_check_peak_process_tree_rss_bytes": warm_rss_bytes,
            "html_bytes": html_bytes,
        },
        "within_target": {
            "cold_analyze_wall_seconds": (
                cold["wall_seconds"]
                <= STANDARD_TARGETS["cold_analyze_wall_seconds_max"]
            ),
            "cold_analyze_peak_process_tree_rss_bytes": (
                cold_rss_bytes
                <= STANDARD_TARGETS[
                    "cold_analyze_peak_process_tree_rss_bytes_max"
                ]
            ),
            "warm_check_wall_seconds": (
                warm["wall_seconds"]
                <= STANDARD_TARGETS["warm_check_wall_seconds_max"]
            ),
            "warm_check_peak_process_tree_rss_bytes": (
                warm_rss_bytes
                <= STANDARD_TARGETS[
                    "warm_check_peak_process_tree_rss_bytes_max"
                ]
            ),
            "html_bytes": html_bytes <= STANDARD_TARGETS["html_bytes_max"],
        },
        "gating": False,
        "note": (
            "Engineering target observations are recorded but are not correctness "
            "gates on shared CI hardware."
        ),
    }


def run_acceptance(
    *,
    binary: Path,
    output_dir: Path,
    mode: str,
) -> dict[str, Any]:
    if mode not in MODES:
        raise ValueError(f"unknown performance fixture mode: {mode}")
    binary = binary.resolve()
    if not binary.is_file():
        raise FileNotFoundError(f"Ferric Lens binary not found: {binary}")

    output_dir = output_dir.resolve()
    output_dir.mkdir(parents=True, exist_ok=True)
    fixture_module = _fixture_module()
    source_multiplier, history_multiplier = MODES[mode]

    with tempfile.TemporaryDirectory(prefix="ferric-lens-perf-") as directory:
        fixture_root = Path(directory) / "repo"
        fixture_manifest = fixture_module.build_fixture(
            fixture_root,
            source_line_target=fixture_module.source_line_target(source_multiplier),
            history_commits=fixture_module.history_commit_count(history_multiplier),
        )
        baseline = str(fixture_manifest["baseline_sha"])

        cache_root = fixture_root / ".ferric-lens" / "cache"
        shutil.rmtree(cache_root, ignore_errors=True)

        cold_json = output_dir / "cold-analyze.json"
        cold_html = output_dir / "cold-analyze.html"
        cold = measure_command(
            [
                str(binary),
                "analyze",
                str(fixture_root),
                "--base",
                baseline,
                "--json",
                str(cold_json),
                "--html",
                str(cold_html),
            ],
            cwd=fixture_root,
            stdout_path=output_dir / "cold-analyze.stdout.txt",
            stderr_path=output_dir / "cold-analyze.stderr.txt",
        )
        if cold["exit_code"] != 0:
            raise RuntimeError(
                "cold analyze failed; see cold-analyze.stderr.txt "
                f"(exit {cold['exit_code']})"
            )

        cache_bytes_after_cold = directory_size(cache_root)
        html_bytes = cold_html.stat().st_size
        html_parse = _parse_html(cold_html)

        edited = fixture_root / "crates" / "perf_crate_00" / "src" / "m00.rs"
        _replace_first_line(edited, "// warm-check local edit")

        warm = measure_command(
            [
                str(binary),
                "check",
                str(fixture_root),
                "--base",
                "HEAD",
                "--json",
                str(output_dir / "warm-check.json"),
            ],
            cwd=fixture_root,
            stdout_path=output_dir / "warm-check.stdout.txt",
            stderr_path=output_dir / "warm-check.stderr.txt",
        )
        if warm["exit_code"] != 0:
            raise RuntimeError(
                "warm check failed; see warm-check.stderr.txt "
                f"(exit {warm['exit_code']})"
            )

        cache_bytes_after_warm = directory_size(cache_root)

        result: dict[str, Any] = {
            "schema_version": SCHEMA_VERSION,
            "mode": mode,
            "fixture": fixture_manifest,
            "environment": _environment(binary),
            "cold_analyze": cold,
            "warm_check": warm,
            "cache_bytes_after_cold": cache_bytes_after_cold,
            "cache_bytes_after_warm": cache_bytes_after_warm,
            "cold_html_bytes": html_bytes,
            "cold_json_bytes": cold_json.stat().st_size,
            "html_parse_proxy": html_parse,
            "browser_dom_open_cost": {
                "status": "not_measured",
                "reason": (
                    "No browser runtime is a Ferric Lens dependency. Record a browser "
                    "measurement separately for release acceptance."
                ),
            },
        }
        if mode == "standard":
            result["engineering_target_observations"] = (
                _standard_target_observations(
                    cold=cold,
                    warm=warm,
                    html_bytes=html_bytes,
                )
            )
        else:
            result["engineering_target_observations"] = {
                "gating": False,
                "note": "Standard-fixture targets are not applied to stress fixtures.",
            }

    result_path = output_dir / "performance-acceptance.json"
    result_path.write_text(
        json.dumps(result, sort_keys=True, indent=2) + "\n",
        encoding="utf-8",
    )
    return result


def _parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=(
            "Run Ferric Lens performance acceptance and record non-gating "
            "resource observations."
        )
    )
    parser.add_argument(
        "--binary",
        type=Path,
        default=Path("target/release/ferric-lens"),
    )
    parser.add_argument(
        "--output-dir",
        type=Path,
        default=Path("target/perf-acceptance"),
    )
    parser.add_argument(
        "--mode",
        choices=sorted(MODES),
        default="standard",
    )
    return parser.parse_args(argv)


def main(argv: list[str]) -> int:
    args = _parse_args(argv)
    result = run_acceptance(
        binary=args.binary,
        output_dir=args.output_dir,
        mode=args.mode,
    )
    print(json.dumps(result, sort_keys=True, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
