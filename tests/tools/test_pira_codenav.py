#!/usr/bin/env python3
"""Black-box contract tests for the read-only pira_codenav CLI."""

from __future__ import annotations

import hashlib
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import unittest


REPO_ROOT = Path(__file__).resolve().parents[2]
RESOURCE_ROOT = REPO_ROOT / "tests" / "resources" / "pira_codenav"
SYNTHETIC_ROOT = RESOURCE_ROOT / "synthetic"
REAL_ROOT = RESOURCE_ROOT / "real"
DEFAULT_BINARY = REPO_ROOT / "tools" / "target" / "debug" / "pira_codenav"
SELECTOR_RE = re.compile(r"selector=(pira://[^\s]+)")


def binary_path() -> Path:
    value = os.environ.get("PIRA_CODENAV_BIN")
    return Path(value).expanduser().resolve() if value else DEFAULT_BINARY


def tree_digest(root: Path) -> str:
    digest = hashlib.sha256()
    for path in sorted(root.rglob("*"), key=lambda item: item.as_posix()):
        relative = path.relative_to(root).as_posix().encode()
        if path.is_symlink():
            digest.update(b"L\0" + relative + b"\0" + os.readlink(path).encode())
        elif path.is_dir():
            digest.update(b"D\0" + relative + b"\0")
        elif path.is_file():
            digest.update(b"F\0" + relative + b"\0")
            digest.update(path.read_bytes())
    return digest.hexdigest()


class PiraCodeNavTests(unittest.TestCase):
    maxDiff = None

    @classmethod
    def setUpClass(cls) -> None:
        cls.binary = binary_path()
        if not cls.binary.is_file():
            raise AssertionError(
                f"pira_codenav binary missing: {cls.binary}; build it or set PIRA_CODENAV_BIN"
            )

    def run_cli(
        self,
        *args: str,
        cwd: Path = SYNTHETIC_ROOT,
        expected: int = 0,
    ) -> subprocess.CompletedProcess[str]:
        result = subprocess.run(
            [str(self.binary), *args],
            cwd=cwd,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(
            expected,
            result.returncode,
            f"command: {args}\nstdout:\n{result.stdout}\nstderr:\n{result.stderr}",
        )
        return result

    @staticmethod
    def line_of(path: Path, needle: str) -> int:
        for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            if needle in line:
                return number
        raise AssertionError(f"missing {needle!r} in {path}")

    def test_global_help_and_languages_are_small_and_focused(self) -> None:
        help_result = self.run_cli("--help")
        self.assertIn("read-only structural", help_result.stdout)
        self.assertIn("outline", help_result.stdout)
        self.assertIn("dependents", help_result.stdout)
        self.assertIn("deps", help_result.stdout)
        self.assertIn("TYPICAL FLOW", help_result.stdout)
        self.assertIn("--max-items 200", help_result.stdout)
        for removed in ("references", "callers", "definition"):
            self.assertNotIn(f"  {removed} ", help_result.stdout)

        languages = self.run_cli("languages")
        self.assertIn(
            "# pira_codenav languages count=17 parser=native "
            "capabilities=outline,show,map,imports,dependents,deps",
            languages.stdout,
        )
        listed = set(languages.stdout.splitlines()[1:])
        self.assertEqual(listed, {
            "python",
            "rust",
            "java",
            "c",
            "cpp",
            "cuda",
            "bash",
            "go",
            "javascript",
            "typescript",
            "csharp",
            "powershell",
            "php",
            "kotlin",
            "lua",
            "hcl",
            "r",
        })

        for command in (
            "outline",
            "show",
            "map",
            "imports",
            "dependents",
            "deps",
            "languages",
        ):
            detailed = self.run_cli(command, "--help")
            self.assertIn("WHEN TO USE", detailed.stdout)
            self.assertIn("USAGE", detailed.stdout)
            self.assertLess(len(detailed.stdout.encode()), 2_000)
        self.assertEqual(
            self.run_cli("languages", "--help").stdout,
            self.run_cli("help", "languages").stdout,
        )
        invalid_languages = self.run_cli("languages", "unexpected", expected=2)
        self.assertIn("accepts no arguments", invalid_languages.stderr)

        show_help = self.run_cli("show", "--help")
        self.assertIn("smallest enclosing named item", show_help.stdout)
        self.assertIn("FILE:START-END", show_help.stdout)
        outline_help = self.run_cli("outline", "--help")
        self.assertIn("--signatures", outline_help.stdout)
        self.assertIn("--match", outline_help.stdout)
        imports_help = self.run_cli("help", "imports")
        self.assertIn("Never invokes a package manager", imports_help.stdout)
        deps_help = self.run_cli("help", "deps")
        self.assertIn("transitive", deps_help.stdout)

    def test_python_outline_auto_detects_and_omits_bodies(self) -> None:
        result = self.run_cli("outline", "python_project/package/api.py")
        self.assertIn("language=python", result.stdout)
        self.assertRegex(result.stdout, r"class\s+Client\b")
        self.assertRegex(result.stdout, r"method\s+Client\.fetch\b")
        self.assertRegex(result.stdout, r"function\s+parse_payload\b")
        self.assertRegex(result.stdout, r"function\s+résumé\b")
        self.assertNotIn("signature=", result.stdout)
        self.assertNotIn("read_text(encoding", result.stdout)
        self.assertNotIn("selector=", result.stdout)
        source_size = (SYNTHETIC_ROOT / "python_project" / "package" / "api.py").stat().st_size
        self.assertLess(len(result.stdout.encode()), source_size)

        signatures = self.run_cli(
            "outline",
            "python_project/package/api.py",
            "--signatures",
            "--max-items",
            "3",
        )
        self.assertIn("signature=", signatures.stdout)
        self.assertIn("class Client", signatures.stdout)

    def test_rust_outline_explicit_language_and_nested_impl(self) -> None:
        result = self.run_cli("rust", "outline", "rust_project/src/parser.rs")
        self.assertIn("language=rust", result.stdout)
        self.assertRegex(result.stdout, r"enum\s+ParseError\b")
        self.assertRegex(result.stdout, r"variant\s+ParseError::Empty\b")
        self.assertRegex(result.stdout, r"struct\s+Parser\b")
        self.assertRegex(result.stdout, r"field\s+Parser::root\b")
        self.assertRegex(result.stdout, r"method\s+Parser::parse\b")
        self.assertRegex(result.stdout, r"function\s+résumé\b")
        self.assertNotIn("Parse one input record and preserve", result.stdout)

    def test_cuda_outline_includes_and_dependents(self) -> None:
        root = SYNTHETIC_ROOT / "cuda_project"
        outline = self.run_cli("outline", "src/kernel.cu", cwd=root)
        self.assertIn("language=cuda", outline.stdout)
        self.assertRegex(outline.stdout, r"function\s+kernels::scale_kernel\b")
        self.assertRegex(outline.stdout, r"function\s+kernels::launch_scale\b")

        header = self.run_cli("outline", "include/kernel.cuh", cwd=root)
        self.assertRegex(header.stdout, r"struct\s+ScaleConfig\b")
        self.assertRegex(header.stdout, r"field\s+ScaleConfig::factor\b")

        imports = self.run_cli("imports", "src/kernel.cu", cwd=root)
        self.assertIn('include/kernel.cuh', imports.stdout)
        self.assertIn("resolution=structural", imports.stdout)
        dependents = self.run_cli("dependents", "include/kernel.cuh", cwd=root)
        self.assertIn("src/kernel.cu", dependents.stdout)

    def test_cuda_macro_recovery_preserves_exact_source(self) -> None:
        root = SYNTHETIC_ROOT / "cuda_project"
        outline = self.run_cli("outline", "src/macro_kernel.cu", cwd=root)
        self.assertIn("parse=recovered", outline.stdout)
        self.assertRegex(outline.stdout, r"function\s+annotated_kernel\b")
        self.assertIn("parse recovered", outline.stderr)

        shown = self.run_cli("show", "src/macro_kernel.cu::annotated_kernel", cwd=root)
        self.assertIn("__global__ void annotated_kernel", shown.stdout)
        self.assertIn("PIRA_DEVICE_LAMBDA", shown.stdout)
        self.assertIn("RESTRICT", shown.stdout)

    def test_go_outline_and_imports(self) -> None:
        root = SYNTHETIC_ROOT / "go_project"
        outline = self.run_cli("outline", "model/user.go", cwd=root)
        self.assertIn("parse=recovered", outline.stdout)
        self.assertRegex(outline.stdout, r"binding\s+DefaultName\b")
        self.assertRegex(outline.stdout, r"binding\s+MaxUsers\b")
        self.assertRegex(outline.stdout, r"binding\s+DefaultNamePointer\b")
        self.assertRegex(outline.stdout, r"interface\s+Labeler\b")
        self.assertRegex(outline.stdout, r"method\s+Labeler\.Label\b")
        self.assertRegex(outline.stdout, r"struct\s+User\b")
        self.assertRegex(outline.stdout, r"field\s+User\.Name\b")
        self.assertRegex(outline.stdout, r"function\s+NewUser\b")
        self.assertRegex(outline.stdout, r"method\s+User\.Label\b")

        imports = self.run_cli("imports", "main.go", cwd=root)
        self.assertIn("external:fmt", imports.stdout)
        self.assertIn("external:example.test/pira/model", imports.stdout)

    def test_javascript_outline_imports_and_dependents(self) -> None:
        root = SYNTHETIC_ROOT / "javascript_project"
        outline = self.run_cli("outline", "lib/model.js", cwd=root)
        self.assertRegex(outline.stdout, r"class\s+User\b")
        self.assertRegex(outline.stdout, r"method\s+User\.label\b")
        self.assertRegex(outline.stdout, r"function\s+normalizeName\b")

        app = self.run_cli("outline", "app.js", cwd=root)
        self.assertRegex(app.stdout, r"binding\s+DEFAULT_NAME\b")
        imports = self.run_cli("imports", "app.js", cwd=root)
        self.assertIn("target=lib/model.js", imports.stdout)
        dependents = self.run_cli("dependents", "lib/model.js", cwd=root)
        self.assertIn("app.js", dependents.stdout)

    def test_typescript_and_tsx_outline_imports_and_dependents(self) -> None:
        root = SYNTHETIC_ROOT / "typescript_project"
        outline = self.run_cli("outline", "model.ts", cwd=root)
        self.assertIn("parse=recovered", outline.stdout)
        self.assertRegex(outline.stdout, r"type\s+UserId\b")
        self.assertRegex(outline.stdout, r"type\s+TrackedUser\b")
        self.assertRegex(outline.stdout, r"interface\s+InvariantBox\b")
        self.assertRegex(outline.stdout, r"enum\s+Status\b")
        self.assertRegex(outline.stdout, r"variant\s+Status\.Disabled\b")
        self.assertRegex(outline.stdout, r"interface\s+User\b")
        self.assertRegex(outline.stdout, r"class\s+Store\b")
        self.assertRegex(outline.stdout, r"method\s+Store\.add\b")

        tsx = self.run_cli("outline", "view.tsx", cwd=root)
        self.assertIn("language=typescript", tsx.stdout)
        self.assertRegex(tsx.stdout, r"function\s+UserName\b")

        nested = self.run_cli("outline", "app.ts", cwd=root)
        self.assertRegex(nested.stdout, r"function\s+register\.validate\b")
        self.assertRegex(nested.stdout, r"function\s+register\.normalized\b")
        shown = self.run_cli("show", "app.ts::register.validate", cwd=root)
        self.assertIn("function validate", shown.stdout)
        self.assertNotIn("const normalized", shown.stdout)

        imports = self.run_cli("imports", "app.ts", cwd=root)
        self.assertIn("target=model.ts", imports.stdout)
        dependents = self.run_cli("dependents", "model.ts", cwd=root)
        self.assertIn("app.ts", dependents.stdout)
        self.assertIn("view.tsx", dependents.stdout)

    def test_csharp_outline_and_imports(self) -> None:
        root = SYNTHETIC_ROOT / "csharp_project"
        outline = self.run_cli("outline", "Program.cs", cwd=root)
        self.assertRegex(outline.stdout, r"namespace\s+Pira\.App\b")
        self.assertRegex(outline.stdout, r"class\s+Pira\.App\.Program\b")
        self.assertRegex(outline.stdout, r"field\s+Pira\.App\.Program\.DefaultUser\b")
        self.assertRegex(outline.stdout, r"method\s+Pira\.App\.Program\.Main\b")

        record = self.run_cli("outline", "Models/User.cs", cwd=root)
        self.assertRegex(record.stdout, r"record\s+Pira\.Models\.User\b")
        self.assertRegex(record.stdout, r"property\s+Pira\.Models\.User\.Label\b")
        imports = self.run_cli("imports", "Program.cs", cwd=root)
        self.assertIn("external:System", imports.stdout)
        self.assertIn("external:Pira.Models", imports.stdout)

        recovery = self.run_cli("outline", "Models/Recovery.cs", cwd=root)
        self.assertIn("parse=recovered", recovery.stdout)
        self.assertRegex(recovery.stdout, r"struct\s+Pira\.Models\.RefBox\b")
        self.assertRegex(recovery.stdout, r"constructor\s+Pira\.Models\.RefBox\.RefBox\b")
        self.assertRegex(recovery.stdout, r"method\s+Pira\.Models\.RefBox\.Identity\b")
        self.assertRegex(recovery.stdout, r"method\s+Pira\.Models\.RefBox\.Read\b")
        self.assertRegex(recovery.stdout, r"class\s+Pira\.Models\.RefBoxExtensions\b")
        self.assertRegex(
            recovery.stdout,
            r"property\s+Pira\.Models\.RefBoxExtensions\.IsValid\b",
        )
        self.assertIn("operator Pira.Models.RefBoxExtensions.operator+", recovery.stdout)
        self.assertNotRegex(recovery.stdout, r"class\s+Pira\.Models\.RefBoxExtensions\.i\b")
        shown = self.run_cli("show", "Models/Recovery.cs::RefBox.Read", cwd=root)
        self.assertIn("return *(T*)pointer;", shown.stdout)

    def test_powershell_outline_imports_and_dependents(self) -> None:
        root = SYNTHETIC_ROOT / "powershell_project"
        outline = self.run_cli("outline", "module.psm1", cwd=root)
        self.assertRegex(outline.stdout, r"enum\s+Mode\b")
        self.assertRegex(outline.stdout, r"class\s+Widget\b")
        self.assertRegex(outline.stdout, r"method\s+Widget::Label\b")
        self.assertRegex(outline.stdout, r"function\s+Get-Widget\b")
        imports = self.run_cli("imports", "app.ps1", cwd=root)
        self.assertIn("target=module.psm1", imports.stdout)
        dependents = self.run_cli("dependents", "module.psm1", cwd=root)
        self.assertIn("dependent=app.ps1", dependents.stdout)

    def test_php_outline_imports_and_dependents(self) -> None:
        root = SYNTHETIC_ROOT / "php_project"
        outline = self.run_cli("outline", "src/Model.php", cwd=root)
        self.assertRegex(outline.stdout, r"trait\s+App\\Named\b")
        self.assertRegex(outline.stdout, r"interface\s+App\\Labelled\b")
        self.assertRegex(outline.stdout, r"enum\s+App\\State\b")
        self.assertRegex(outline.stdout, r"method\s+App\\Model::label\b")
        imports = self.run_cli("imports", "app.php", cwd=root)
        self.assertIn("target=src/Model.php", imports.stdout)
        self.assertIn("external:App\\Model", imports.stdout)
        dependents = self.run_cli("dependents", "src/Model.php", cwd=root)
        self.assertIn("dependent=app.php", dependents.stdout)

    def test_kotlin_outline_imports_and_dependents(self) -> None:
        root = SYNTHETIC_ROOT / "kotlin_project"
        outline = self.run_cli("outline", "src/main/kotlin/example/Model.kt", cwd=root)
        self.assertRegex(outline.stdout, r"enum\s+State\b")
        self.assertRegex(outline.stdout, r"interface\s+Labelled\b")
        self.assertRegex(outline.stdout, r"method\s+Model\.label\b")
        self.assertRegex(outline.stdout, r"type\s+ModelFactory\b")
        imports = self.run_cli(
            "imports", "src/main/kotlin/example/App.kt", cwd=root
        )
        self.assertIn("target=src/main/kotlin/example/Model.kt", imports.stdout)
        dependents = self.run_cli(
            "dependents", "src/main/kotlin/example/Model.kt", cwd=root
        )
        self.assertIn("dependent=src/main/kotlin/example/App.kt", dependents.stdout)

    def test_lua_outline_imports_and_dependents(self) -> None:
        root = SYNTHETIC_ROOT / "lua_project"
        outline = self.run_cli("outline", "lib/model.lua", cwd=root)
        self.assertRegex(outline.stdout, r"function\s+normalize\b")
        self.assertRegex(outline.stdout, r"function\s+M\.new\b")
        self.assertRegex(outline.stdout, r"function\s+M\.version\b")
        imports = self.run_cli("imports", "app.lua", cwd=root)
        self.assertIn("target=lib/model.lua", imports.stdout)
        dependents = self.run_cli("dependents", "lib/model.lua", cwd=root)
        self.assertIn("dependent=app.lua", dependents.stdout)

    def test_hcl_outline_imports_and_dependents(self) -> None:
        root = SYNTHETIC_ROOT / "hcl_project"
        outline = self.run_cli("outline", "main.tf", cwd=root)
        self.assertRegex(outline.stdout, r"block\s+module\.child\b")
        self.assertRegex(outline.stdout, r"block\s+resource\.example_widget\.main\b")
        self.assertRegex(
            outline.stdout,
            r"attribute\s+resource\.example_widget\.main\.lifecycle\.prevent_destroy\b",
        )
        imports = self.run_cli("imports", "main.tf", cwd=root)
        self.assertIn("target=child/main.tf", imports.stdout)
        dependents = self.run_cli("dependents", "child/main.tf", cwd=root)
        self.assertIn("dependent=main.tf", dependents.stdout)

    def test_r_outline_imports_and_dependents(self) -> None:
        root = SYNTHETIC_ROOT / "r_project"
        outline = self.run_cli("outline", "helpers.R", cwd=root)
        self.assertRegex(outline.stdout, r"function\s+normalize_name\b")
        self.assertRegex(outline.stdout, r"function\s+normalize_name\.trim\b")
        self.assertRegex(outline.stdout, r"function\s+double_value\b")
        imports = self.run_cli("imports", "app.R", cwd=root)
        self.assertIn("target=helpers.R", imports.stdout)
        self.assertIn("external:stats", imports.stdout)
        dependents = self.run_cli("dependents", "helpers.R", cwd=root)
        self.assertIn("dependent=app.R", dependents.stdout)

    def test_extensionless_python_uses_shebang(self) -> None:
        result = self.run_cli("outline", "extensionless_python")
        self.assertIn("language=python", result.stdout)
        self.assertRegex(result.stdout, r"function\s+from_shebang\b")

    def test_java_outline_imports_and_dependents(self) -> None:
        root = SYNTHETIC_ROOT / "java_project"
        outline = self.run_cli("outline", "src/com/example/App.java", cwd=root)
        self.assertRegex(outline.stdout, r"class\s+App\b")
        self.assertRegex(outline.stdout, r"field\s+App\.user\b")
        self.assertRegex(outline.stdout, r"constructor\s+App\.App\b")
        self.assertRegex(outline.stdout, r"method\s+App\.names\b")

        imports = self.run_cli("imports", "src/com/example/App.java", cwd=root)
        self.assertIn("import com.example.model.User", imports.stdout)
        self.assertIn("target=src/com/example/model/User.java", imports.stdout)
        self.assertIn("external:java", imports.stdout)

        dependents = self.run_cli(
            "dependents", "src/com/example/model/User.java", cwd=root
        )
        self.assertIn("src/com/example/App.java", dependents.stdout)

    def test_c_outline_includes_and_dependents(self) -> None:
        root = SYNTHETIC_ROOT / "c_project"
        outline = self.run_cli("outline", "src/app.c", cwd=root)
        self.assertRegex(outline.stdout, r"function\s+twice\b")
        self.assertRegex(outline.stdout, r"function\s+main\b")

        header = self.run_cli("c", "outline", "include/model.h", cwd=root)
        self.assertRegex(header.stdout, r"struct\s+Model\b")
        self.assertRegex(header.stdout, r"field\s+Model::value\b")
        self.assertRegex(header.stdout, r"function\s+model_value\b")

        imports = self.run_cli("imports", "src/app.c", cwd=root)
        self.assertIn('#include \\"../include/model.h\\"', imports.stdout)
        self.assertIn("target=include/model.h", imports.stdout)
        self.assertIn("external:stdio.h", imports.stdout)

        dependents = self.run_cli("c", "dependents", "include/model.h", cwd=root)
        self.assertIn("src/app.c", dependents.stdout)
        self.assertIn("src/model.c", dependents.stdout)

    def test_cpp_outline_includes_and_dependents(self) -> None:
        root = SYNTHETIC_ROOT / "cpp_project"
        outline = self.run_cli("outline", "include/widget.hpp", cwd=root)
        self.assertRegex(outline.stdout, r"namespace\s+demo\b")
        self.assertRegex(outline.stdout, r"class\s+demo::Widget\b")
        self.assertRegex(outline.stdout, r"constructor\s+demo::Widget::Widget\b")
        self.assertRegex(outline.stdout, r"method\s+demo::Widget::name\b")
        self.assertRegex(outline.stdout, r"field\s+demo::Widget::name_\b")

        shown = self.run_cli("show", "include/widget.hpp:9", cwd=root)
        self.assertIn("const std::string& name() const;", shown.stdout)

        imports = self.run_cli("imports", "src/app.cpp", cwd=root)
        self.assertIn('#include \\"../include/widget.hpp\\"', imports.stdout)
        self.assertIn("target=include/widget.hpp", imports.stdout)

        dependents = self.run_cli("dependents", "include/widget.hpp", cwd=root)
        self.assertIn("src/app.cpp", dependents.stdout)
        self.assertIn("src/widget.cpp", dependents.stdout)

    def test_bash_outline_source_and_shebang_detection(self) -> None:
        root = SYNTHETIC_ROOT / "bash_project"
        outline = self.run_cli("outline", "app.sh", cwd=root)
        self.assertRegex(outline.stdout, r"function\s+main\b")

        imports = self.run_cli("imports", "app.sh", cwd=root)
        self.assertIn("lib.sh", imports.stdout)
        self.assertIn("target=lib.sh", imports.stdout)

        dependents = self.run_cli("dependents", "lib.sh", cwd=root)
        self.assertIn("app.sh", dependents.stdout)

        extensionless = self.run_cli("outline", "extensionless_bash")
        self.assertIn("language=bash", extensionless.stdout)
        self.assertRegex(extensionless.stdout, r"function\s+from_shebang\b")

    def test_real_world_additional_languages_have_useful_outlines(self) -> None:
        cases = [
            ("java_junit/StringUtils.java", "java", r"class\s+StringUtils\b"),
            ("c_jq/main.c", "c", r"function\s+main\b"),
            ("bash_bats/bats.sh", "bash", r"function\s+bats_tee\b"),
            (
                "powershell_powershell/ResxGen.psm1",
                "powershell",
                r"class\s+EventMessage\b",
            ),
            (
                "php_laravel/Collection.php",
                "php",
                r"class\s+Illuminate\\Support\\Collection\b",
            ),
            (
                "kotlin_coroutines/CoroutineDispatcher.kt",
                "kotlin",
                r"class\s+CoroutineDispatcher\b",
            ),
            ("lua_neovim/lsp.lua", "lua", r"function\s+lsp\.start\b"),
            (
                "hcl_terraform/root.tf",
                "hcl",
                r"block\s+resource\.test_thing\.source\b",
            ),
            ("r_dplyr/mutate.R", "r", r"function\s+mutate\.data\.frame\b"),
        ]
        for relative, language, expected in cases:
            with self.subTest(language=language):
                result = self.run_cli(language, "outline", str(REAL_ROOT / relative))
                self.assertRegex(result.stdout, expected)

        cpp = self.run_cli("cpp", "outline", str(REAL_ROOT / "cpp_fmt/format.cc"))
        self.assertRegex(cpp.stdout, r"namespace\s+detail\b")
        self.assertRegex(cpp.stdout, r"(?:method|function)\s+detail::.*to_decimal\b")

    def test_show_by_position_returns_smallest_named_item(self) -> None:
        source = SYNTHETIC_ROOT / "python_project" / "package" / "api.py"
        line = self.line_of(source, "payload =")
        result = self.run_cli("show", f"python_project/package/api.py:{line}")
        self.assertIn("untrusted repository source", result.stdout)
        self.assertNotIn("controls_escaped=0", result.stdout)
        self.assertIn("def fetch(self, relative: str)", result.stdout)
        self.assertIn("payload =", result.stdout)
        self.assertNotIn("def parse_payload", result.stdout)
        self.assertIn("--- end source ---", result.stdout)

    def test_show_exact_line_range_is_bounded_and_validated(self) -> None:
        result = self.run_cli("show", "python_project/package/api.py:18-20")
        self.assertIn("item=lines:18-20 kind=range", result.stdout.splitlines()[0])
        self.assertIn("def fetch", result.stdout)
        self.assertIn("payload =", result.stdout)
        self.assertNotIn("class Client", result.stdout)
        self.assertNotIn("return parse_payload", result.stdout)

        reversed_range = self.run_cli(
            "show", "python_project/package/api.py:20-18", expected=2
        )
        self.assertIn("1 <= START <= END", reversed_range.stderr)
        clamped = self.run_cli("show", "python_project/package/api.py:29-999")
        self.assertIn("item=lines:29-31", clamped.stdout.splitlines()[0])
        self.assertIn("def résumé", clamped.stdout)
        beyond_start = self.run_cli(
            "show", "python_project/package/api.py:999-1000", expected=2
        )
        self.assertIn("starts at 999", beyond_start.stderr)

    def test_show_uses_column_to_disambiguate_same_line_items(self) -> None:
        with tempfile.TemporaryDirectory(prefix="pira-codenav-column-") as temp:
            root = Path(temp)
            (root / "same_line.cpp").write_text(
                "int alpha(); int beta();\n", encoding="utf-8"
            )
            result = self.run_cli("show", "same_line.cpp:1:20", cwd=root)
            self.assertIn("item=beta", result.stdout.splitlines()[0])
            self.assertIn("int beta();", result.stdout)
            self.assertNotIn("int alpha();", result.stdout)

    def test_show_preserves_attached_python_decorators(self) -> None:
        result = self.run_cli(
            "show", "python_project/package/models.py:4"
        )
        self.assertIn("@dataclass(frozen=True)", result.stdout)
        self.assertIn("class User:", result.stdout)

    def test_show_accepts_bounded_multiple_targets_and_deduplicates(self) -> None:
        with tempfile.TemporaryDirectory(prefix="pira-codenav-show-many-") as temp:
            root = Path(temp)
            (root / "sample.py").write_text(
                "def alpha():\n    return 1\n\n"
                "def beta():\n    return 2\n",
                encoding="utf-8",
            )
            result = self.run_cli(
                "show",
                "sample.py:1",
                "sample.py:1",
                "sample.py:4",
                "--max-items",
                "2",
                cwd=root,
            )
            header = result.stdout.splitlines()[0]
            self.assertIn("targets=3", header)
            self.assertIn("shown=2", header)
            self.assertIn("duplicates=1", header)
            self.assertIn("def alpha():", result.stdout)
            self.assertIn("def beta():", result.stdout)

            limited = self.run_cli(
                "show",
                "sample.py:1",
                "sample.py:4",
                "--max-items",
                "1",
                cwd=root,
            )
            self.assertIn("shown=1", limited.stdout.splitlines()[0])
            self.assertIn("omitted=1", limited.stdout.splitlines()[0])
            self.assertIn("def alpha():", limited.stdout)
            self.assertNotIn("def beta():", limited.stdout)

    def test_show_byte_limit_omits_whole_items(self) -> None:
        with tempfile.TemporaryDirectory(prefix="pira-codenav-show-bytes-") as temp:
            root = Path(temp)
            (root / "sample.py").write_text(
                "def long_item():\n    return '" + ("x" * 256) + "'\n",
                encoding="utf-8",
            )
            result = self.run_cli(
                "show", "sample.py:1", "--max-bytes", "100", cwd=root
            )
            header = result.stdout.splitlines()[0]
            self.assertIn("shown=0", header)
            self.assertIn("byte_limited=1", header)
            self.assertNotIn("def long_item", result.stdout)

            item_limit_only = self.run_cli(
                "show", "sample.py:1", "--max-items", "1", cwd=root
            )
            self.assertTrue(
                item_limit_only.stdout.startswith("# pira_codenav show file=")
            )
            self.assertIn("def long_item", item_limit_only.stdout)

    def test_outline_selector_round_trips_and_detects_staleness(self) -> None:
        with tempfile.TemporaryDirectory(prefix="pira-codenav-selector-") as temp:
            root = Path(temp)
            shutil.copytree(SYNTHETIC_ROOT / "python_project", root, dirs_exist_ok=True)
            outline = self.run_cli("outline", "package/api.py", "--selectors", cwd=root)
            selector = next(
                match.group(1)
                for line in outline.stdout.splitlines()
                if "Client.fetch" in line and (match := SELECTOR_RE.search(line))
            )
            shown = self.run_cli("show", selector, cwd=root)
            self.assertIn("def fetch", shown.stdout)

            path = root / "package" / "api.py"
            path.write_text(
                path.read_text(encoding="utf-8").replace(
                    "payload = (self.root", "payload = (self.root.resolve()"
                ),
                encoding="utf-8",
            )
            stale = self.run_cli("show", selector, cwd=root, expected=4)
            self.assertIn("stale selector", stale.stderr.lower())

    def test_all_overloaded_selectors_round_trip_to_distinct_items(self) -> None:
        source = REAL_ROOT / "python_click" / "decorators.py"
        outline = self.run_cli("outline", str(source), "--selectors")
        selectors = [
            match.group(1)
            for line in outline.stdout.splitlines()
            if re.match(r"^function\s+command\s", line)
            and (match := SELECTOR_RE.search(line))
        ]
        self.assertEqual(5, len(selectors))
        hashes = set()
        for selector in selectors:
            shown = self.run_cli("show", selector)
            match = re.search(r"\bhash=([0-9a-f]{16})", shown.stdout.splitlines()[0])
            self.assertIsNotNone(match)
            hashes.add(match.group(1))
        self.assertEqual(5, len(hashes))

    def test_ambiguous_qualified_name_requires_location_or_selector(self) -> None:
        source = REAL_ROOT / "python_click" / "decorators.py"
        result = self.run_cli("show", f"{source}::command", expected=3)
        self.assertIn("ambiguous symbol", result.stderr)
        self.assertIn(":137", result.stderr)
        self.assertIn(":168", result.stderr)
        self.assertIn("location or selector", result.stderr)

    def test_exact_qualified_name_wins_over_longer_suffix(self) -> None:
        with tempfile.TemporaryDirectory(prefix="pira-codenav-exact-name-") as temp:
            root = Path(temp)
            (root / "clients.py").write_text(
                "class Client:\n"
                "    def request(self):\n"
                "        return 'sync'\n\n"
                "class AsyncClient:\n"
                "    def request(self):\n"
                "        return 'async'\n",
                encoding="utf-8",
            )
            result = self.run_cli("show", "clients.py::Client.request", cwd=root)
            self.assertIn("item=Client.request", result.stdout.splitlines()[0])
            self.assertIn("return 'sync'", result.stdout)
            self.assertNotIn("return 'async'", result.stdout)

    def test_multi_show_keeps_valid_targets_when_one_fails(self) -> None:
        with tempfile.TemporaryDirectory(prefix="pira-codenav-partial-show-") as temp:
            root = Path(temp)
            (root / "sample.py").write_text(
                "def alpha(): return 1\n\ndef beta(): return 2\n",
                encoding="utf-8",
            )
            result = self.run_cli(
                "show",
                "sample.py::alpha",
                "sample.py::missing",
                "sample.py::beta",
                cwd=root,
            )
            header = result.stdout.splitlines()[0]
            self.assertIn("shown=2", header)
            self.assertIn("failed=1", header)
            self.assertIn("def alpha", result.stdout)
            self.assertIn("def beta", result.stdout)
            self.assertRegex(
                result.stdout,
                r'error target="sample\.py::missing" code=3 message="symbol not found',
            )

            failed = self.run_cli(
                "show",
                "sample.py::missing",
                "sample.py::also_missing",
                cwd=root,
                expected=3,
            )
            self.assertIn("shown=0", failed.stdout.splitlines()[0])
            self.assertIn("failed=2", failed.stdout.splitlines()[0])
            self.assertIn("all show targets failed", failed.stderr)

    def test_multi_outline_and_imports_keep_valid_files(self) -> None:
        outline = self.run_cli(
            "outline",
            "python_project/package/api.py",
            "missing.py",
        )
        self.assertIn("class Client", outline.stdout)
        self.assertRegex(
            outline.stdout,
            r'outline error file="missing\.py" code=2 message="cannot inspect',
        )

        imports = self.run_cli(
            "imports",
            "javascript_project/app.js",
            "missing.js",
        )
        self.assertIn("target=javascript_project/lib/model.js", imports.stdout)
        self.assertRegex(
            imports.stdout,
            r'imports error file="missing\.js" code=2 message="cannot inspect',
        )

        failed = self.run_cli(
            "outline",
            "missing.py",
            "also_missing.py",
            expected=3,
        )
        self.assertEqual(2, failed.stdout.count("outline error file="))
        self.assertIn("all outline files failed", failed.stderr)

    def test_python_module_bindings_are_navigable(self) -> None:
        with tempfile.TemporaryDirectory(prefix="pira-codenav-bindings-") as temp:
            root = Path(temp)
            (root / "config.py").write_text(
                "TimeoutTypes = tuple[float, float]\n"
                "DEFAULT_TIMEOUT_CONFIG = TimeoutTypes\n"
                "private_value = 3\n",
                encoding="utf-8",
            )
            outline = self.run_cli("outline", "config.py", cwd=root)
            for name in ("TimeoutTypes", "DEFAULT_TIMEOUT_CONFIG", "private_value"):
                self.assertRegex(outline.stdout, rf"binding\s+{name}\b")

            shown = self.run_cli("show", "config.py::TimeoutTypes", cwd=root)
            self.assertIn("kind=binding", shown.stdout.splitlines()[0])
            self.assertIn("TimeoutTypes = tuple[float, float]", shown.stdout)
            self.assertNotIn("DEFAULT_TIMEOUT_CONFIG", shown.stdout)

    def test_map_prioritizes_declarations_over_module_bindings(self) -> None:
        with tempfile.TemporaryDirectory(prefix="pira-codenav-map-bindings-") as temp:
            root = Path(temp)
            bindings = "".join(f"value_{index} = {index}\n" for index in range(20))
            (root / "module.py").write_text(
                bindings + "\nclass Important: pass\n", encoding="utf-8"
            )
            result = self.run_cli("map", ".", cwd=root)
            self.assertIn("Important", result.stdout)

    def test_outline_match_filters_without_losing_exact_locations(self) -> None:
        with tempfile.TemporaryDirectory(prefix="pira-codenav-outline-match-") as temp:
            root = Path(temp)
            (root / "sample.py").write_text(
                "class Client:\n"
                "    def request(self): return 1\n"
                "    def close(self): return None\n\n"
                "class AsyncClient:\n"
                "    def request(self): return 3\n\n"
                "def helper(): return 2\n",
                encoding="utf-8",
            )
            result = self.run_cli(
                "outline", "sample.py", "--match", "Client.request", cwd=root
            )
            self.assertIn("matched=1", result.stdout.splitlines()[0])
            self.assertRegex(result.stdout, r"method\s+Client\.request\b")
            self.assertNotIn("AsyncClient.request", result.stdout)
            self.assertNotIn("Client.close", result.stdout)
            self.assertNotIn("helper", result.stdout)
            unfiltered = self.run_cli("outline", "sample.py", cwd=root)
            self.assertLess(len(result.stdout), len(unfiltered.stdout))

            repeated = self.run_cli(
                "outline",
                "sample.py",
                "--match",
                "Client.request",
                "--match",
                "helper",
                cwd=root,
            )
            self.assertIn("matched=2", repeated.stdout.splitlines()[0])
            self.assertIn("Client.request", repeated.stdout)
            self.assertIn("helper", repeated.stdout)

    def test_command_errors_point_to_valid_target_forms(self) -> None:
        with tempfile.TemporaryDirectory(prefix="pira-codenav-guidance-") as temp:
            root = Path(temp)
            (root / "sample.py").write_text("def sample(): pass\n", encoding="utf-8")
            show = self.run_cli(
                "show", "sample.py", "--symbol", "sample", cwd=root, expected=2
            )
            self.assertIn("direct target", show.stderr)
            self.assertIn("show --help", show.stderr)

            deps = self.run_cli("deps", "sample.py:1", cwd=root, expected=2)
            self.assertIn("file path, not a source location", deps.stderr)
            self.assertIn("deps --help", deps.stderr)

    def test_map_is_mixed_language_bounded_and_gitignore_aware(self) -> None:
        result = self.run_cli("map", ".", "--max-items", "50")
        self.assertIn("language=python", result.stdout)
        self.assertIn("language=rust", result.stdout)
        self.assertIn("omitted=", result.stdout)
        self.assertNotIn("ignored_generated.py", result.stdout)
        self.assertNotIn("target/", result.stdout)

        rust_only = self.run_cli("rust", "map", ".", "--max-items", "200")
        self.assertIn("rust_project/src/parser.rs", rust_only.stdout)
        self.assertNotIn("python_project/package/api.py", rust_only.stdout)

    def test_map_reports_discovery_failures_and_balances_directories(self) -> None:
        with tempfile.TemporaryDirectory(prefix="pira-codenav-map-") as temp:
            root = Path(temp)
            for directory, count in (("a", 5), ("b", 1), ("c", 1)):
                (root / directory).mkdir()
                for index in range(count):
                    (root / directory / f"item_{index}.py").write_text(
                        f"def {directory}_{index}(): pass\n", encoding="utf-8"
                    )
            (root / "broken.py").write_bytes(b"\xff")
            (root / "ambiguous.h").write_text("int value;\n", encoding="utf-8")
            (root / "notes.txt").write_text("documentation\n", encoding="utf-8")

            result = self.run_cli("map", ".", "--max-items", "3", cwd=root)
            header = result.stdout.splitlines()[0]
            for expected in (
                "discovered=10",
                "eligible=8",
                "parsed=7",
                "ok=7",
                "recovered=0",
                "partial=0",
                "failed=1",
                "unsupported=1",
                "ambiguous=1",
                "shown=3",
                "omitted=4",
            ):
                self.assertIn(expected, header)
            shown = result.stdout.splitlines()[1:]
            self.assertTrue(any("file=a/" in line for line in shown))
            self.assertTrue(any("file=b/" in line for line in shown))
            self.assertTrue(any("file=c/" in line for line in shown))

    def test_map_compacts_pathological_symbol_names(self) -> None:
        with tempfile.TemporaryDirectory(prefix="pira-codenav-map-name-") as temp:
            root = Path(temp)
            identifier = "a" * 5000
            (root / "long.py").write_text(
                f"def {identifier}(): pass\n", encoding="utf-8"
            )
            result = self.run_cli("map", ".", cwd=root)
            self.assertIn("…", result.stdout)
            self.assertLess(len(result.stdout.encode()), 1000)

    def test_python_src_layout_resolves_absolute_local_imports(self) -> None:
        with tempfile.TemporaryDirectory(prefix="pira-codenav-python-src-") as temp:
            root = Path(temp)
            package = root / "src" / "acme"
            package.mkdir(parents=True)
            (package / "__init__.py").write_text("", encoding="utf-8")
            (package / "models.py").write_text(
                "class Model: pass\n", encoding="utf-8"
            )
            (package / "api.py").write_text(
                "from acme.models import Model\n", encoding="utf-8"
            )

            imports = self.run_cli("imports", "src/acme/api.py", cwd=root)
            self.assertIn("target=src/acme/models.py", imports.stdout)
            self.assertIn("resolution=structural", imports.stdout)

            dependents = self.run_cli(
                "dependents", "src/acme/models.py", cwd=root
            )
            self.assertIn("src/acme/api.py", dependents.stdout)

            graph = self.run_cli(
                "deps",
                "src/acme/models.py",
                "--direction",
                "dependents",
                cwd=root,
            )
            self.assertIn("from=src/acme/api.py", graph.stdout)

    def test_python_duplicate_import_roots_are_reported_as_ambiguous(self) -> None:
        with tempfile.TemporaryDirectory(prefix="pira-codenav-python-roots-") as temp:
            root = Path(temp)
            for prefix in (root, root / "src"):
                package = prefix / "acme"
                package.mkdir(parents=True)
                (package / "models.py").write_text(
                    "class Model: pass\n", encoding="utf-8"
                )
            (root / "app.py").write_text(
                "from acme.models import Model\n", encoding="utf-8"
            )
            result = self.run_cli("imports", "app.py", cwd=root)
            self.assertIn("target=ambiguous:acme", result.stdout)
            self.assertIn("resolution=ambiguous", result.stdout)

    def test_python_imports_and_reverse_dependents(self) -> None:
        imports = self.run_cli("imports", "package/api.py", cwd=SYNTHETIC_ROOT / "python_project")
        self.assertIn("from .models import User, normalize_name", imports.stdout)
        self.assertIn("target=package/models.py", imports.stdout)
        self.assertIn("resolution=structural", imports.stdout)
        self.assertIn("target=external:json", imports.stdout)

        dependents = self.run_cli(
            "dependents", "package/models.py", cwd=SYNTHETIC_ROOT / "python_project"
        )
        self.assertIn("package/api.py", dependents.stdout)
        self.assertIn("package/models.py", dependents.stdout)

        rooted = self.run_cli(
            "dependents",
            "package/models.py",
            "--root",
            "python_project",
            cwd=SYNTHETIC_ROOT,
        )
        self.assertIn("package/api.py", rooted.stdout)
        self.assertIn("target=package/models.py", rooted.stdout)

    def test_transitive_file_dependencies_are_bounded_and_directional(self) -> None:
        root = SYNTHETIC_ROOT / "python_project"
        reverse = self.run_cli(
            "deps",
            "package/models.py",
            "--direction",
            "dependents",
            "--depth",
            "2",
            cwd=root,
        )
        self.assertIn("direction=dependents", reverse.stdout.splitlines()[0])
        self.assertRegex(
            reverse.stdout,
            r"edge depth=1 direction=dependent from=package/api\.py to=package/models\.py",
        )
        self.assertRegex(
            reverse.stdout,
            r"edge depth=2 direction=dependent from=app\.py to=package/api\.py",
        )

        direct = self.run_cli(
            "deps",
            "package/models.py",
            "--direction",
            "dependents",
            "--depth",
            "1",
            cwd=root,
        )
        self.assertIn("package/api.py", direct.stdout)
        self.assertNotIn("from=app.py", direct.stdout)

        forward = self.run_cli(
            "deps", "app.py", "--direction", "imports", "--depth", "2", cwd=root
        )
        self.assertRegex(
            forward.stdout,
            r"edge depth=1 direction=import from=app\.py to=package/api\.py",
        )
        self.assertRegex(
            forward.stdout,
            r"edge depth=2 direction=import from=package/api\.py to=package/models\.py",
        )

    def test_rust_imports_and_reverse_dependents(self) -> None:
        imports = self.run_cli("imports", "src/lib.rs", cwd=SYNTHETIC_ROOT / "rust_project")
        self.assertIn("pub mod parser", imports.stdout)
        self.assertIn("target=src/parser.rs", imports.stdout)

        dependents = self.run_cli(
            "dependents", "src/parser.rs", cwd=SYNTHETIC_ROOT / "rust_project"
        )
        self.assertIn("src/lib.rs", dependents.stdout)
        self.assertIn("target=src/parser.rs", dependents.stdout)

    def test_real_world_python_and_rust_files_have_useful_outlines(self) -> None:
        python = self.run_cli("outline", str(REAL_ROOT / "python_click" / "decorators.py"))
        self.assertIn("language=python", python.stdout)
        self.assertRegex(python.stdout, r"function\s+command\b")
        self.assertRegex(python.stdout, r"function\s+pass_context\.new_func\b")
        self.assertNotRegex(python.stdout, r"method\s+pass_context\.new_func\b")

        rust = self.run_cli("outline", str(REAL_ROOT / "rust_ripgrep" / "gitignore.rs"))
        self.assertIn("language=rust", rust.stdout)
        self.assertRegex(rust.stdout, r"struct\s+Gitignore\b")
        self.assertRegex(rust.stdout, r"method\s+Gitignore::matched\b")

    def test_closed_output_consumer_does_not_panic(self) -> None:
        process = subprocess.Popen(
            [
                str(self.binary),
                "outline",
                str(REAL_ROOT / "python_click" / "decorators.py"),
            ],
            cwd=SYNTHETIC_ROOT,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        assert process.stdout is not None
        self.assertTrue(process.stdout.readline().startswith("# pira_codenav outline"))
        process.stdout.close()
        assert process.stderr is not None
        stderr = process.stderr.read()
        process.stderr.close()
        return_code = process.wait(timeout=10)
        self.assertEqual(0, return_code, stderr)
        self.assertNotIn("panicked", stderr)

    def test_malformed_file_returns_partial_outline_with_warning(self) -> None:
        result = self.run_cli("outline", "malformed.py")
        self.assertRegex(result.stdout, r"parse=(partial|error)")
        self.assertRegex(result.stdout, r"class\s+Incomplete\b")
        self.assertIn("parse", result.stderr.lower())

    def test_unknown_language_and_explicit_mismatch_fail_concisely(self) -> None:
        unknown = self.run_cli("outline", "unknown.source", expected=2)
        self.assertIn("cannot determine language", unknown.stderr.lower())
        self.assertIn("pira_codenav LANGUAGE outline", unknown.stderr)

        mismatch = self.run_cli("rust", "outline", "python_project/app.py", expected=2)
        self.assertIn("language mismatch", mismatch.stderr.lower())

    def test_removed_lsp_duplicates_are_not_commands(self) -> None:
        for command in ("symbols", "definition", "references", "calls", "callers"):
            result = self.run_cli(command, "anything", expected=2)
            self.assertIn("unknown subcommand", result.stderr.lower())

    def test_all_successful_queries_are_workspace_read_only(self) -> None:
        with tempfile.TemporaryDirectory(prefix="pira-codenav-readonly-") as temp:
            root = Path(temp)
            shutil.copytree(SYNTHETIC_ROOT, root, dirs_exist_ok=True)
            before = tree_digest(root)
            commands = (
                ("outline", "python_project/package/api.py"),
                ("show", "python_project/package/api.py:11"),
                (
                    "show",
                    "python_project/package/api.py:11",
                    "python_project/package/models.py:4",
                ),
                ("map", ".", "--max-items", "30"),
                ("imports", "rust_project/src/lib.rs"),
                ("dependents", "rust_project/src/parser.rs"),
                (
                    "deps",
                    "python_project/package/models.py",
                    "--direction",
                    "dependents",
                ),
            )
            for command in commands:
                self.run_cli(*command, cwd=root)
            self.assertEqual(before, tree_digest(root))


if __name__ == "__main__":
    unittest.main(verbosity=2)
