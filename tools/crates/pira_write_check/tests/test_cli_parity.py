"""Native CLI/prototype parity, not a semantic-quality benchmark."""
from __future__ import annotations
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[4]
PROTOTYPE = ROOT / "tools/dev/pira_write_check/check.py"
HAVE_PROTOTYPE = PROTOTYPE.is_file() and PROTOTYPE.with_name("lexicon.json").is_file()
if HAVE_PROTOTYPE:
    SPEC = importlib.util.spec_from_file_location("write_prototype", PROTOTYPE)
    prototype = importlib.util.module_from_spec(SPEC)
    SPEC.loader.exec_module(prototype)
BINARY = Path(os.environ.get("PIRA_WRITE_CHECK_BIN", ROOT / "tools/target/debug/pira_write_check"))

@unittest.skipUnless(HAVE_PROTOTYPE, "optional retained Python prototype is absent; Rust fixtures remain self-contained")
class NativeParityTests(unittest.TestCase):
    def test_json_and_text_parity_with_prototype_on_general_sources(self):
        inventory = json.loads(prototype.LEXICON_PATH.read_text(encoding="utf-8"))
        texts = [
            "The detector records six separate pulses.\n",
            "😀\tNOT necessarily equal; although exact, only three remain.\r\n",
            "`never`\n> perhaps\nThe interval is likely finite.\n",
            "_not not_ πnot not² not\u0301 \u0301not; lıKely ſlightly İS\n",
            "hold\u2028without\u0085assumptions\u001dhowever\r\n",
            "\n".join(f"({marker}); {marker.upper()}!" for markers in inventory.values() for marker in markers),
        ]
        with tempfile.TemporaryDirectory(prefix="write-parity-") as temporary:
            source = Path(temporary) / "α file.md"
            for text in texts:
                source.write_bytes(text.encode("utf-8"))
                before = source.read_bytes(), source.stat().st_mtime_ns
                for options in ([], ["--json"]):
                    with self.subTest(text=text[:70], options=options):
                        native = subprocess.run([str(BINARY), *options, str(source)], capture_output=True, timeout=10)
                        python = subprocess.run([os.sys.executable, "-B", str(PROTOTYPE), *options, str(source)], capture_output=True, timeout=10)
                        self.assertEqual(native.returncode, 0, native.stderr)
                        self.assertEqual(python.returncode, 0, python.stderr)
                        self.assertEqual(native.stderr, b"")
                        if options:
                            self.assertEqual(json.loads(native.stdout), json.loads(python.stdout))
                        else:
                            # Python stdout translates LF to CRLF on Windows; Rust emits LF.
                            self.assertEqual(native.stdout.replace(b"\r\n", b"\n"),
                                             python.stdout.replace(b"\r\n", b"\n"))
                self.assertEqual((source.read_bytes(), source.stat().st_mtime_ns), before)

    def test_custom_literal_overlap_parity(self):
        with tempfile.TemporaryDirectory(prefix="write-parity-") as temporary:
            root = Path(temporary)
            lexicon = root / "lexicon.json"
            lexicon.write_text(json.dumps({"z": ["a[b]c", "go go", "x-y"], "a": ["go", "x"]}), encoding="utf-8")
            source = root / "source.txt"
            source.write_bytes(b"A[B]C abc go\tgo go go x-y")
            args = ["--json", "--lexicon", str(lexicon), str(source)]
            native = subprocess.run([str(BINARY), *args], capture_output=True, timeout=10)
            python = subprocess.run([os.sys.executable, "-B", str(PROTOTYPE), *args], capture_output=True, timeout=10)
            self.assertEqual(native.returncode, 0, native.stderr)
            self.assertEqual(python.returncode, 0, python.stderr)
            self.assertEqual(json.loads(native.stdout), json.loads(python.stdout))

if __name__ == "__main__":
    unittest.main()
