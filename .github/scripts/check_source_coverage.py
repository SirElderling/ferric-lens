#!/usr/bin/env python3
"""Enforce source-centric LLVM coverage for Ferric Lens production Rust code."""

from __future__ import annotations

import json
from pathlib import Path
import sys
from typing import Any


def _relative_production_file(filename: str, repo_root: Path) -> str | None:
    path = Path(filename)
    try:
        relative = path.resolve().relative_to(repo_root.resolve())
    except ValueError:
        return None
    parts = relative.parts
    if not parts or parts[0] != "src":
        return None
    if relative.name.endswith("_tests.rs"):
        return None
    return relative.as_posix()


def source_coverage(document: dict[str, Any], repo_root: Path) -> dict[str, Any]:
    data = document["data"][0]
    region_counts: dict[tuple[Any, ...], int] = {}
    function_counts: dict[tuple[Any, ...], int] = {}

    for function in data.get("functions", []):
        filenames = function.get("filenames", [])
        production_regions: list[tuple[Any, ...]] = []

        for region in function.get("regions", []):
            start_line, start_col, end_line, end_col, count, file_id, _, kind = region
            if file_id >= len(filenames):
                continue
            relative = _relative_production_file(filenames[file_id], repo_root)
            if relative is None:
                continue
            key = (relative, start_line, start_col, end_line, end_col, kind)
            region_counts[key] = max(region_counts.get(key, 0), int(count))
            production_regions.append(
                (relative, start_line, start_col, end_line, end_col, kind)
            )

        if production_regions:
            function_key = tuple(production_regions)
            function_counts[function_key] = max(
                function_counts.get(function_key, 0), int(function.get("count", 0))
            )

    executable_lines: set[tuple[str, int]] = set()
    covered_lines: set[tuple[str, int]] = set()
    for (relative, start_line, _, end_line, _, _), count in region_counts.items():
        for line in range(start_line, end_line + 1):
            key = (relative, line)
            executable_lines.add(key)
            if count > 0:
                covered_lines.add(key)

    missed_regions = sorted(key for key, count in region_counts.items() if count == 0)
    missed_functions = sorted(key for key, count in function_counts.items() if count == 0)
    missed_lines = sorted(executable_lines - covered_lines)

    return {
        "lines_total": len(executable_lines),
        "lines_covered": len(executable_lines) - len(missed_lines),
        "regions_total": len(region_counts),
        "regions_covered": len(region_counts) - len(missed_regions),
        "functions_total": len(function_counts),
        "functions_covered": len(function_counts) - len(missed_functions),
        "missed_lines": missed_lines,
        "missed_regions": missed_regions,
        "missed_functions": missed_functions,
    }


def _percent(covered: int, total: int) -> float:
    return 100.0 if total == 0 else covered * 100.0 / total


def main(argv: list[str]) -> int:
    if len(argv) not in (2, 3):
        print(
            "usage: check_source_coverage.py <coverage.json> [repo-root]",
            file=sys.stderr,
        )
        return 2

    coverage_path = Path(argv[1])
    repo_root = Path(argv[2]) if len(argv) == 3 else Path.cwd()
    document = json.loads(coverage_path.read_text(encoding="utf-8"))
    result = source_coverage(document, repo_root)

    for name in ("lines", "regions", "functions"):
        covered = result[f"{name}_covered"]
        total = result[f"{name}_total"]
        print(f"{name}: {covered}/{total} ({_percent(covered, total):.2f}%)")

    if result["missed_lines"]:
        print("uncovered source lines:")
        for relative, line in result["missed_lines"]:
            print(f"  {relative}:{line}")

    if result["missed_regions"]:
        print("uncovered source regions:")
        for relative, sl, sc, el, ec, _ in result["missed_regions"]:
            print(f"  {relative}:{sl}:{sc}-{el}:{ec}")

    if result["missed_functions"]:
        print(f"uncovered source functions: {len(result['missed_functions'])}")

    return 1 if (
        result["missed_lines"]
        or result["missed_regions"]
        or result["missed_functions"]
    ) else 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
