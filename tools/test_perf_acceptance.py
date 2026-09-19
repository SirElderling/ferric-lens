import importlib.util
from pathlib import Path
import sys
import tempfile
import unittest


SCRIPT = Path(__file__).with_name("perf_acceptance.py")


def load_module():
    spec = importlib.util.spec_from_file_location("perf_acceptance", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


class PerformanceAcceptanceTests(unittest.TestCase):
    def test_process_tree_rss_sums_root_and_descendants_only(self):
        module = load_module()
        snapshot = """10 1 100
11 10 200
12 11 300
20 1 400
"""

        self.assertEqual(module.process_tree_rss_kib(snapshot, 10), 600)
        self.assertEqual(module.process_tree_rss_kib(snapshot, 20), 400)
        self.assertEqual(module.process_tree_rss_kib(snapshot, 99), 0)

    def test_directory_size_counts_regular_files(self):
        module = load_module()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "nested").mkdir()
            (root / "a").write_bytes(b"abc")
            (root / "nested" / "b").write_bytes(b"12345")

            self.assertEqual(module.directory_size(root), 8)
            self.assertEqual(module.directory_size(root / "missing"), 0)

    def test_wait4_usage_rss_provides_a_final_sample_floor(self):
        module = load_module()

        class Usage:
            ru_maxrss = 1234

        self.assertGreater(module._usage_max_rss_kib(Usage()), 0)

    def test_measure_command_records_exit_wall_cpu_and_output(self):
        module = load_module()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            measurement = module.measure_command(
                [
                    sys.executable,
                    "-c",
                    "import sys, time; print('ok'); print('err', file=sys.stderr); time.sleep(0.03)",
                ],
                cwd=root,
                stdout_path=root / "stdout.txt",
                stderr_path=root / "stderr.txt",
                sample_interval=0.005,
            )

            self.assertEqual(measurement["exit_code"], 0)
            self.assertGreater(measurement["wall_seconds"], 0)
            self.assertGreaterEqual(measurement["child_cpu_seconds"], 0)
            self.assertGreater(measurement["peak_process_tree_rss_kib"], 0)
            self.assertEqual((root / "stdout.txt").read_text().strip(), "ok")
            self.assertEqual((root / "stderr.txt").read_text().strip(), "err")


if __name__ == "__main__":
    unittest.main()
