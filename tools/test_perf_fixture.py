import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest


SCRIPT = Path(__file__).with_name("perf_fixture.py")


def load_module():
    spec = importlib.util.spec_from_file_location("perf_fixture", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


def git(root: Path, *args: str) -> str:
    output = subprocess.run(
        ["git", *args],
        cwd=root,
        check=True,
        text=True,
        capture_output=True,
    )
    return output.stdout.strip()


def production_lines(root: Path) -> int:
    return sum(
        len(path.read_text(encoding="utf-8").splitlines())
        for path in sorted(root.glob("crates/*/src/*.rs"))
    )


class PerformanceFixtureTests(unittest.TestCase):
    def test_small_fixture_has_exact_requested_source_size_and_history(self):
        module = load_module()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "fixture"
            manifest = module.build_fixture(
                root,
                crate_count=2,
                modules_per_crate=2,
                source_line_target=82,
                history_commits=3,
            )

            self.assertEqual(production_lines(root), 82)
            self.assertEqual(manifest["production_rust_lines"], 82)
            self.assertEqual(manifest["crate_count"], 2)
            self.assertEqual(manifest["history_commits_after_baseline"], 3)
            self.assertEqual(git(root, "rev-list", "--count", "HEAD"), "4")
            self.assertEqual(git(root, "status", "--porcelain"), "")
            self.assertTrue((root / "Cargo.lock").is_file())

    def test_fixture_generation_is_reproducible(self):
        module = load_module()
        with tempfile.TemporaryDirectory() as directory:
            first = Path(directory) / "first"
            second = Path(directory) / "second"

            first_manifest = module.build_fixture(
                first,
                crate_count=2,
                modules_per_crate=2,
                source_line_target=82,
                history_commits=2,
            )
            second_manifest = module.build_fixture(
                second,
                crate_count=2,
                modules_per_crate=2,
                source_line_target=82,
                history_commits=2,
            )

            self.assertEqual(first_manifest, second_manifest)
            self.assertEqual(
                git(first, "rev-parse", "HEAD"),
                git(second, "rev-parse", "HEAD"),
            )
            first_files = {
                path.relative_to(first): path.read_bytes()
                for path in first.rglob("*")
                if path.is_file() and ".git" not in path.parts
            }
            second_files = {
                path.relative_to(second): path.read_bytes()
                for path in second.rglob("*")
                if path.is_file() and ".git" not in path.parts
            }
            self.assertEqual(first_files, second_files)

    def test_standard_and_source_stress_targets_are_exact(self):
        module = load_module()

        self.assertEqual(module.source_line_target(1), 100_000)
        self.assertEqual(module.source_line_target(10), 1_000_000)
        self.assertEqual(module.history_commit_count(1), 200)
        self.assertEqual(module.history_commit_count(10), 2_000)


if __name__ == "__main__":
    unittest.main()
