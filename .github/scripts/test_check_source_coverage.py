import importlib.util
from pathlib import Path
import tempfile
import unittest


SCRIPT = Path(__file__).with_name("check_source_coverage.py")
SPEC = importlib.util.spec_from_file_location("check_source_coverage", SCRIPT)
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(MODULE)


def function(filename, count, regions):
    return {
        "name": "symbol",
        "count": count,
        "filenames": [filename],
        "regions": regions,
        "branches": [],
        "mcdc_records": [],
    }


class SourceCoverageTests(unittest.TestCase):
    def test_unions_duplicate_compilation_mappings_by_source_region(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "src" / "lib.rs"
            document = {
                "data": [{
                    "functions": [
                        function(str(source), 0, [[1, 1, 1, 8, 0, 0, 0, 0]]),
                        function(str(source), 1, [[1, 1, 1, 8, 1, 0, 0, 0]]),
                    ]
                }]
            }

            result = MODULE.source_coverage(document, root)

            self.assertEqual(result["regions_total"], 1)
            self.assertEqual(result["regions_covered"], 1)
            self.assertEqual(result["functions_total"], 1)
            self.assertEqual(result["functions_covered"], 1)
            self.assertEqual(result["missed_lines"], [])

    def test_reports_distinct_uncovered_source_regions_and_lines(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "src" / "lib.rs"
            document = {
                "data": [{
                    "functions": [
                        function(
                            str(source),
                            1,
                            [
                                [1, 1, 1, 8, 1, 0, 0, 0],
                                [2, 1, 2, 8, 0, 0, 0, 0],
                            ],
                        )
                    ]
                }]
            }

            result = MODULE.source_coverage(document, root)

            self.assertEqual(result["regions_covered"], 1)
            self.assertEqual(result["regions_total"], 2)
            self.assertEqual(result["missed_lines"], [("src/lib.rs", 2)])
            self.assertEqual(len(result["missed_regions"]), 1)

    def test_excludes_test_modules_and_files_outside_src(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            test_module = root / "src" / "lib_tests.rs"
            integration = root / "tests" / "contract.rs"
            document = {
                "data": [{
                    "functions": [
                        function(str(test_module), 0, [[1, 1, 1, 8, 0, 0, 0, 0]]),
                        function(str(integration), 0, [[1, 1, 1, 8, 0, 0, 0, 0]]),
                    ]
                }]
            }

            result = MODULE.source_coverage(document, root)

            self.assertEqual(result["regions_total"], 0)
            self.assertEqual(result["functions_total"], 0)
            self.assertEqual(result["lines_total"], 0)


if __name__ == "__main__":
    unittest.main()
