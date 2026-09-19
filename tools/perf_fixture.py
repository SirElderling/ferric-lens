#!/usr/bin/env python3
"""Generate reproducible Ferric Lens release-acceptance repositories."""

from __future__ import annotations

import argparse
from datetime import datetime, timedelta, timezone
import json
from pathlib import Path
import shutil
import subprocess
import sys
from typing import TextIO


GENERATOR_SCHEMA_VERSION = 1
STANDARD_CRATE_COUNT = 20
STANDARD_MODULES_PER_CRATE = 10
STANDARD_SOURCE_LINES = 100_000
STANDARD_HISTORY_COMMITS = 200
SUPPORTED_MULTIPLIERS = (1, 10)
FIXTURE_NAME = "ferric-lens-perf-fixture"
FIXTURE_EMAIL = "fixture@ferric-lens.invalid"
BASE_TIME = datetime(2001, 1, 1, tzinfo=timezone.utc)


def source_line_target(multiplier: int) -> int:
    _validate_multiplier(multiplier)
    return STANDARD_SOURCE_LINES * multiplier


def history_commit_count(multiplier: int) -> int:
    _validate_multiplier(multiplier)
    return STANDARD_HISTORY_COMMITS * multiplier


def _validate_multiplier(multiplier: int) -> None:
    if multiplier not in SUPPORTED_MULTIPLIERS:
        raise ValueError(
            f"unsupported multiplier {multiplier}; expected one of {SUPPORTED_MULTIPLIERS}"
        )


def _run(
    args: list[str],
    *,
    cwd: Path,
    env: dict[str, str] | None = None,
    input_text: str | None = None,
) -> str:
    completed = subprocess.run(
        args,
        cwd=cwd,
        env=env,
        input=input_text,
        text=True,
        capture_output=True,
    )
    if completed.returncode != 0:
        command = " ".join(args)
        raise RuntimeError(
            f"{command} failed with exit code {completed.returncode}: "
            f"{completed.stderr.strip()}"
        )
    return completed.stdout.strip()


def _git(root: Path, *args: str) -> str:
    return _run(["git", *args], cwd=root)


def _crate_name(index: int) -> str:
    return f"perf_crate_{index:02d}"


def _module_name(index: int) -> str:
    return f"m{index:02d}"


def _function_block(index: int) -> list[str]:
    return [
        f"pub fn f_{index:05d}(value: usize) -> usize {{",
        "    if value % 2 == 0 {",
        "        value.wrapping_add(1)",
        "    } else {",
        "        value",
        "    }",
        "}",
    ]


def _layout(
    *,
    crate_count: int,
    modules_per_crate: int,
    source_line_target: int,
) -> tuple[list[int], list[int]]:
    if crate_count <= 0 or modules_per_crate <= 0:
        raise ValueError("crate_count and modules_per_crate must be positive")

    module_count = crate_count * modules_per_crate
    fixed_lines = crate_count * modules_per_crate + module_count * 2
    if source_line_target < fixed_lines:
        raise ValueError(
            f"source_line_target {source_line_target} is below the "
            f"{fixed_lines}-line fixture minimum"
        )

    variable_lines = source_line_target - fixed_lines
    total_functions, padding = divmod(variable_lines, 7)
    base_functions, extra_functions = divmod(total_functions, module_count)

    functions = [
        base_functions + (1 if index < extra_functions else 0)
        for index in range(module_count)
    ]
    padding_lines = [0] * module_count
    for index in range(padding):
        padding_lines[index] += 1

    return functions, padding_lines


def _module_source(
    *,
    crate_index: int,
    module_index: int,
    modules_per_crate: int,
    function_count: int,
    padding_lines: int,
    revision: int | None = None,
) -> str:
    if revision is not None:
        first_line = f"// fixture revision {revision:06d}"
    else:
        first_line = (
            f"// deterministic fixture crate={crate_index:02d} "
            f"module={module_index:02d}"
        )

    neighbor = (module_index + 1) % modules_per_crate
    lines = [
        first_line,
        f"use crate::{_module_name(neighbor)} as neighbor;",
    ]
    for index in range(function_count):
        lines.extend(_function_block(index))
    for index in range(padding_lines):
        lines.append(f"// exact-line padding {index}")

    return "\n".join(lines) + "\n"


def _write_workspace(
    root: Path,
    *,
    crate_count: int,
    modules_per_crate: int,
    source_line_target: int,
) -> tuple[list[int], list[int]]:
    functions, padding = _layout(
        crate_count=crate_count,
        modules_per_crate=modules_per_crate,
        source_line_target=source_line_target,
    )

    members = [f'    "crates/{_crate_name(index)}",' for index in range(crate_count)]
    (root / "Cargo.toml").write_text(
        "[workspace]\n"
        'resolver = "2"\n'
        "members = [\n"
        + "\n".join(members)
        + "\n]\n",
        encoding="utf-8",
    )

    flat_index = 0
    for crate_index in range(crate_count):
        crate_root = root / "crates" / _crate_name(crate_index)
        source_root = crate_root / "src"
        source_root.mkdir(parents=True, exist_ok=True)

        manifest = [
            "[package]",
            f'name = "{_crate_name(crate_index)}"',
            'version = "0.1.0"',
            'edition = "2021"',
            "publish = false",
        ]
        if crate_index > 0:
            manifest.extend(
                [
                    "",
                    "[dependencies]",
                    (
                        f'{_crate_name(crate_index - 1)} = '
                        f'{{ path = "../{_crate_name(crate_index - 1)}" }}'
                    ),
                ]
            )
        (crate_root / "Cargo.toml").write_text(
            "\n".join(manifest) + "\n",
            encoding="utf-8",
        )

        declarations = [
            f"pub mod {_module_name(module_index)};"
            for module_index in range(modules_per_crate)
        ]
        (source_root / "lib.rs").write_text(
            "\n".join(declarations) + "\n",
            encoding="utf-8",
        )

        for module_index in range(modules_per_crate):
            revision = 0 if crate_index == 0 and module_index == 0 else None
            content = _module_source(
                crate_index=crate_index,
                module_index=module_index,
                modules_per_crate=modules_per_crate,
                function_count=functions[flat_index],
                padding_lines=padding[flat_index],
                revision=revision,
            )
            (source_root / f"{_module_name(module_index)}.rs").write_text(
                content,
                encoding="utf-8",
            )
            flat_index += 1

    return functions, padding


def _production_rust_lines(root: Path) -> int:
    total = 0
    for path in sorted((root / "crates").glob("*/src/*.rs")):
        total += len(path.read_text(encoding="utf-8").splitlines())
    return total


def _commit_environment(offset: int) -> dict[str, str]:
    instant = BASE_TIME + timedelta(seconds=offset)
    stamp = instant.strftime("%Y-%m-%dT%H:%M:%S+0000")
    env = dict(**__import__("os").environ)
    env.update(
        {
            "GIT_AUTHOR_NAME": "Ferric Lens Fixture",
            "GIT_AUTHOR_EMAIL": FIXTURE_EMAIL,
            "GIT_AUTHOR_DATE": stamp,
            "GIT_COMMITTER_NAME": "Ferric Lens Fixture",
            "GIT_COMMITTER_EMAIL": FIXTURE_EMAIL,
            "GIT_COMMITTER_DATE": stamp,
        }
    )
    return env


def _write_fast_import_commit(
    stream: TextIO,
    *,
    mark: int,
    parent: str,
    revision: int,
    path: str,
    content: str,
) -> None:
    timestamp = int((BASE_TIME + timedelta(seconds=revision)).timestamp())
    message = f"history revision {revision}"
    payload = content.encode("utf-8")
    stream.write("commit refs/heads/main\n")
    stream.write(f"mark :{mark}\n")
    stream.write(
        f"committer Ferric Lens Fixture <{FIXTURE_EMAIL}> {timestamp} +0000\n"
    )
    stream.write(f"data {len(message.encode('utf-8'))}\n{message}\n")
    stream.write(f"from {parent}\n")
    stream.write(f"M 100644 inline {path}\n")
    stream.write(f"data {len(payload)}\n")
    stream.write(payload.decode("ascii"))
    stream.write("\n")
    stream.flush()


def _append_history(
    root: Path,
    *,
    history_commits: int,
    modules_per_crate: int,
    functions: list[int],
    padding: list[int],
    baseline_sha: str,
) -> str:
    if history_commits <= 0:
        return baseline_sha

    first_path = "crates/perf_crate_00/src/m00.rs"
    process = subprocess.Popen(
        ["git", "fast-import", "--quiet"],
        cwd=root,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    assert process.stdin is not None
    parent = baseline_sha

    try:
        for revision in range(1, history_commits + 1):
            content = _module_source(
                crate_index=0,
                module_index=0,
                modules_per_crate=modules_per_crate,
                function_count=functions[0],
                padding_lines=padding[0],
                revision=revision,
            )
            mark = revision
            _write_fast_import_commit(
                process.stdin,
                mark=mark,
                parent=parent,
                revision=revision,
                path=first_path,
                content=content,
            )
            parent = f":{mark}"
        process.stdin.write("done\n")
        process.stdin.close()
        stderr = process.stderr.read() if process.stderr is not None else ""
        return_code = process.wait()
    except BaseException:
        process.kill()
        process.wait()
        raise

    if return_code != 0:
        raise RuntimeError(f"git fast-import failed: {stderr.strip()}")

    _git(root, "reset", "--hard", "-q", "main")
    return _git(root, "rev-parse", "HEAD")


def build_fixture(
    root: Path,
    *,
    crate_count: int = STANDARD_CRATE_COUNT,
    modules_per_crate: int = STANDARD_MODULES_PER_CRATE,
    source_line_target: int = STANDARD_SOURCE_LINES,
    history_commits: int = STANDARD_HISTORY_COMMITS,
) -> dict[str, object]:
    """Create one deterministic fixture repository and return its manifest."""

    root = root.resolve()
    if root.exists():
        if any(root.iterdir()):
            raise ValueError(f"fixture destination is not empty: {root}")
    else:
        root.mkdir(parents=True)

    functions, padding = _write_workspace(
        root,
        crate_count=crate_count,
        modules_per_crate=modules_per_crate,
        source_line_target=source_line_target,
    )

    actual_lines = _production_rust_lines(root)
    if actual_lines != source_line_target:
        raise RuntimeError(
            f"fixture line count mismatch: expected {source_line_target}, "
            f"found {actual_lines}"
        )

    _run(
        ["cargo", "generate-lockfile", "--offline"],
        cwd=root,
        env={**__import__("os").environ, "CARGO_NET_OFFLINE": "true"},
    )

    _git(root, "init", "-q")
    _git(root, "symbolic-ref", "HEAD", "refs/heads/main")
    _git(root, "config", "user.name", "Ferric Lens Fixture")
    _git(root, "config", "user.email", FIXTURE_EMAIL)
    _git(root, "config", "commit.gpgsign", "false")
    _git(root, "config", "core.autocrlf", "false")
    _git(root, "add", ".")
    _run(
        ["git", "commit", "-q", "-m", "baseline"],
        cwd=root,
        env=_commit_environment(0),
    )
    baseline_sha = _git(root, "rev-parse", "HEAD")

    head_sha = _append_history(
        root,
        history_commits=history_commits,
        modules_per_crate=modules_per_crate,
        functions=functions,
        padding=padding,
        baseline_sha=baseline_sha,
    )

    status = _git(root, "status", "--porcelain")
    if status:
        raise RuntimeError(f"fixture repository is unexpectedly dirty: {status}")

    return {
        "schema_version": GENERATOR_SCHEMA_VERSION,
        "crate_count": crate_count,
        "modules_per_crate": modules_per_crate,
        "production_rust_lines": actual_lines,
        "history_commits_after_baseline": history_commits,
        "total_git_commits": history_commits + 1,
        "baseline_sha": baseline_sha,
        "head_sha": head_sha,
    }


def _parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Generate a deterministic Ferric Lens performance fixture."
    )
    parser.add_argument("destination", type=Path)
    parser.add_argument(
        "--source-multiplier",
        type=int,
        choices=SUPPORTED_MULTIPLIERS,
        default=1,
    )
    parser.add_argument(
        "--history-multiplier",
        type=int,
        choices=SUPPORTED_MULTIPLIERS,
        default=1,
    )
    parser.add_argument(
        "--force",
        action="store_true",
        help="remove an existing destination before generating the fixture",
    )
    return parser.parse_args(argv)


def main(argv: list[str]) -> int:
    args = _parse_args(argv)
    destination = args.destination.resolve()
    if destination.exists() and args.force:
        shutil.rmtree(destination)

    manifest = build_fixture(
        destination,
        source_line_target=source_line_target(args.source_multiplier),
        history_commits=history_commit_count(args.history_multiplier),
    )
    print(json.dumps(manifest, sort_keys=True, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
