"""Selection-only regression; actual Windows execution remains a native CI gate."""
import unittest

import native_ci
from native_fixture import windows_verbatim_path


def cases(suite):
    for case in suite:
        if isinstance(case, unittest.TestSuite):
            yield from cases(case)
        else:
            yield case


class NativeSelectionTests(unittest.TestCase):
    def test_windows_fixture_spelling_handles_drive_unc_and_verbatim_paths(self):
        for source, expected in (
            (r"C:\fixture\root", r"\\?\C:\fixture\root"),
            (r"\\server\share\root", r"\\?\UNC\server\share\root"),
            (r"\\?\C:\fixture\root", r"\\?\C:\fixture\root"),
            (r"\\?\UNC\server\share\root", r"\\?\UNC\server\share\root"),
        ):
            with self.subTest(source=source):
                self.assertEqual(windows_verbatim_path(source), expected)

    def test_windows_fixture_spelling_rejects_relative_paths(self):
        for path in ("relative", r"C:relative", r"\rooted"):
            with self.subTest(path=path), self.assertRaises(ValueError):
                windows_verbatim_path(path)

    def test_windows_gate_retains_new_categories_and_launch_bursts(self):
        loader = unittest.TestLoader()
        selected = [case.id() for case in cases(native_ci.windows_suite(loader))]
        self.assertFalse(loader.errors, loader.errors)
        self.assertEqual(len(selected), len(set(selected)), "duplicate native checks")
        for module in ("test_native_auth", "test_provider_config", "test_prerequisite_local", "test_local_usability"):
            expected = {case.id() for case in cases(loader.loadTestsFromName(module))}
            self.assertTrue(expected, module)
            self.assertTrue(expected.issubset(selected), module)
        for name in ("test_100_simultaneous_launches_preserve_independent_lifecycle",
                     "test_100_interleaved_harnesses_preserve_sessions_and_resume"):
            self.assertIn("test_subteams.SubteamTests." + name, selected)


if __name__ == "__main__":
    unittest.main()
