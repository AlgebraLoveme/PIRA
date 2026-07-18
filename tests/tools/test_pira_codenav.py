#!/usr/bin/env python3
"""Black-box contract tests for the read-only pira_codenav CLI."""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import unittest


REPO_ROOT = Path(__file__).resolve().parents[2]
RESOURCE_ROOT = REPO_ROOT / "tests" / "resources" / "pira_codenav"
SYNTHETIC_ROOT = RESOURCE_ROOT / "synthetic"
REAL_ROOT = RESOURCE_ROOT / "real"
DEFAULT_BINARY = REPO_ROOT / "tools" / "target" / "debug" / "pira_codenav"
SELECTOR_RE = re.compile(r"selector=(pira://[^\s]+)")
FAKE_LSP = Path(__file__).with_name("fake_lsp_server.py")


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
    def fake_lsp_args(*extra: str) -> tuple[str, ...]:
        arguments = ["--lsp", sys.executable, "--lsp-arg", str(FAKE_LSP)]
        for value in extra:
            arguments.extend(("--lsp-arg", value))
        return tuple(arguments)

    @staticmethod
    def line_of(path: Path, needle: str) -> int:
        for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            if needle in line:
                return number
        raise AssertionError(f"missing {needle!r} in {path}")

    def test_global_help_and_languages_are_small_and_focused(self) -> None:
        help_result = self.run_cli("--help")
        self.assertIn("read-only code navigation", help_result.stdout)
        self.assertIn("outline", help_result.stdout)
        self.assertIn("find", help_result.stdout)
        self.assertIn("dependents", help_result.stdout)
        self.assertIn("deps", help_result.stdout)
        self.assertIn("--lsp", help_result.stdout)
        self.assertIn("CHOOSE A COMMAND", help_result.stdout)
        self.assertIn("TYPICAL FLOW", help_result.stdout)
        self.assertIn("--max-items 200", help_result.stdout)
        self.assertIn("do not substitute text matching", help_result.stdout)
        self.assertIn("Predictable success fields are omitted", help_result.stdout)
        for added in (
            "definition",
            "implementation",
            "type-definition",
            "references",
            "callers",
            "callees",
            "hover",
        ):
            self.assertIn(f"  {added} ", help_result.stdout)
        for removed in ("calls", "workspace-symbol", "rename"):
            self.assertNotIn(f"  {removed} ", help_result.stdout)

        languages = self.run_cli("languages")
        self.assertEqual(
            "# pira_codenav languages count=23", languages.stdout.splitlines()[0]
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
            "ruby",
            "swift",
            "scala",
            "dart",
            "elixir",
            "julia",
        })

        for command in (
            "outline",
            "show",
            "map",
            "find",
            "imports",
            "dependents",
            "deps",
            "definition",
            "implementation",
            "type-definition",
            "references",
            "callers",
            "callees",
            "hover",
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
        self.assertIn("parser-free", show_help.stdout)
        outline_help = self.run_cli("outline", "--help")
        self.assertIn("--signatures", outline_help.stdout)
        self.assertIn("--match", outline_help.stdout)
        self.assertIn("--lsp [LANGUAGE=]ABSOLUTE_PATH", outline_help.stdout)
        self.assertIn("without implementation bodies", outline_help.stdout)
        imports_help = self.run_cli("help", "imports")
        self.assertIn("Never invokes a package manager", imports_help.stdout)
        self.assertIn("workspace root", imports_help.stdout)
        dependents_help = self.run_cli("dependents", "--help")
        self.assertIn("FILE is relative to --root", dependents_help.stdout)
        deps_help = self.run_cli("help", "deps")
        self.assertIn("transitive", deps_help.stdout)
        self.assertIn("--direction VALUE", deps_help.stdout)
        find_help = self.run_cli("find", "--help")
        self.assertIn("parsed declarations, not text", find_help.stdout)
        self.assertIn("freshness-checked `show` targets", find_help.stdout)
        definition_help = self.run_cli("definition", "--help")
        self.assertIn("--lsp-init", definition_help.stdout)
        self.assertIn("Up to 32 targets reuse", definition_help.stdout)
        self.assertIn("one server and open document per file", definition_help.stdout)
        references_help = self.run_cli("references", "--help")
        self.assertIn("--include-declaration", references_help.stdout)
        self.assertIn("never performs text search", references_help.stdout)
        language_help = self.run_cli("python", "--help")
        self.assertIn("extensionless or ambiguous file", language_help.stdout)
        self.assertIn("restrict map/find to python", language_help.stdout)

    def test_python_outline_auto_detects_and_omits_bodies(self) -> None:
        result = self.run_cli("outline", "python_project/package/api.py")
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
        self.assertRegex(result.stdout, r"enum\s+ParseError\b")
        self.assertRegex(result.stdout, r"variant\s+ParseError::Empty\b")
        self.assertRegex(result.stdout, r"struct\s+Parser\b")
        self.assertRegex(result.stdout, r"field\s+Parser::root\b")
        self.assertRegex(result.stdout, r"method\s+Parser::parse\b")
        self.assertRegex(result.stdout, r"function\s+résumé\b")
        self.assertNotIn("Parse one input record and preserve", result.stdout)

    def test_rust_generic_impl_uses_natural_owner_name(self) -> None:
        with tempfile.TemporaryDirectory(prefix="pira-codenav-rust-generics-") as temp:
            root = Path(temp)
            (root / "cache.rs").write_text(
                "struct Cache<T>(T);\n\n"
                "impl<T> Cache<T> {\n"
                "    fn get(&self) -> &T {\n"
                "        &self.0\n"
                "    }\n"
                "}\n",
                encoding="utf-8",
            )
            outline = self.run_cli("outline", "cache.rs", cwd=root)
            self.assertRegex(outline.stdout, r"method\s+Cache::get\b")
            shown = self.run_cli("show", "cache.rs::Cache::get", cwd=root)
            self.assertIn("fn get(&self) -> &T", shown.stdout)

    def test_cuda_outline_includes_and_dependents(self) -> None:
        root = SYNTHETIC_ROOT / "cuda_project"
        outline = self.run_cli("outline", "src/kernel.cu", cwd=root)
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

    def test_unclean_cuda_requires_lsp(self) -> None:
        root = SYNTHETIC_ROOT / "cuda_project"
        outline = self.run_cli(
            "outline", "src/macro_kernel.cu", cwd=root, expected=3
        )
        self.assertIn("Tree-sitter", outline.stderr)
        self.assertIn("--lsp", outline.stderr)

    def test_go_outline_and_imports(self) -> None:
        root = SYNTHETIC_ROOT / "go_project"
        outline = self.run_cli("outline", "model/user.go", cwd=root, expected=3)
        self.assertIn("--lsp", outline.stderr)

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
        outline = self.run_cli("outline", "model.ts", cwd=root, expected=3)
        self.assertIn("--lsp", outline.stderr)

        tsx = self.run_cli("outline", "view.tsx", cwd=root)
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

        recovery = self.run_cli(
            "outline", "Models/Recovery.cs", cwd=root, expected=3
        )
        self.assertIn("--lsp", recovery.stderr)

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

    def test_next_language_batch_outlines_imports_and_exact_show(self) -> None:
        cases = (
            (
                "ruby_project",
                "model.rb",
                "Demo::User.label",
                "def label",
                "app.rb",
                "target=model.rb",
                "model.rb",
            ),
            (
                "swift_project",
                "Model.swift",
                "User.render",
                "func render",
                "Model.swift",
                "external:Foundation",
                None,
            ),
            (
                "scala_project",
                "Model.scala",
                "Helpers.normalize",
                "def normalize",
                "Model.scala",
                "external:scala.collection.mutable",
                None,
            ),
            (
                "dart_project",
                "model.dart",
                "User.label",
                "String get label",
                "app.dart",
                "target=model.dart",
                "model.dart",
            ),
            (
                "elixir_project",
                "model.ex",
                "Demo.User.label",
                "def label",
                "app.exs",
                "external:Demo.User",
                None,
            ),
            (
                "julia_project",
                "Model.jl",
                "Demo.normalize",
                "function normalize",
                "App.jl",
                "target=Model.jl",
                "Model.jl",
            ),
        )
        for project, source, qualified, source_marker, importer, import_marker, local in cases:
            with self.subTest(project=project):
                root = SYNTHETIC_ROOT / project
                outline = self.run_cli(
                    "outline",
                    source,
                    "--match",
                    qualified,
                    "--selectors",
                    cwd=root,
                )
                self.assertIn(qualified, outline.stdout)
                selector = SELECTOR_RE.search(outline.stdout)
                self.assertIsNotNone(selector)
                shown = self.run_cli("show", selector.group(1), cwd=root)
                self.assertIn(source_marker, shown.stdout)

                imports = self.run_cli("imports", importer, cwd=root)
                self.assertIn(import_marker, imports.stdout)
                if local is not None:
                    dependents = self.run_cli("dependents", local, cwd=root)
                    self.assertIn(f"dependent={importer}", dependents.stdout)
                    dependencies = self.run_cli("deps", importer, cwd=root)
                    self.assertIn(f"to={local}", dependencies.stdout)

    def test_next_language_batch_shebangs_and_explicit_language(self) -> None:
        scripts = (
            ("ruby", "#!/usr/bin/env ruby\ndef hello\n  1\nend\n", "hello"),
            (
                "elixir",
                "#!/usr/bin/env elixir\ndefmodule Demo do\n  def hello, do: 1\nend\n",
                "Demo.hello",
            ),
            (
                "julia",
                "#!/usr/bin/env julia\nfunction hello()\n    1\nend\n",
                "hello",
            ),
        )
        with tempfile.TemporaryDirectory(prefix="pira-codenav-shebang-") as temp:
            root = Path(temp)
            for language, source, expected in scripts:
                with self.subTest(language=language):
                    path = root / f"run-{language}"
                    path.write_text(source, encoding="utf-8")
                    inferred = self.run_cli("outline", path.name, cwd=root)
                    self.assertIn(f"language={language}", inferred.stdout.splitlines()[0])
                    self.assertIn(expected, inferred.stdout)

            explicit = self.run_cli(
                "ruby", "outline", "run-ruby", "--match", "hello", cwd=root
            )
            self.assertIn("function hello", explicit.stdout)

    def test_new_language_can_use_language_qualified_lsp(self) -> None:
        with tempfile.TemporaryDirectory(prefix="pira-codenav-ruby-lsp-") as temp:
            root = Path(temp)
            source = root / "dirty.rb"
            source.write_text("def native\n  1\nend\nbroken = (\n", encoding="utf-8")
            missing = self.run_cli("outline", source.name, cwd=root, expected=3)
            self.assertIn("--lsp ruby=ABSOLUTE_SERVER_PATH", missing.stderr)

            restored = self.run_cli(
                "outline",
                source.name,
                "--lsp",
                f"ruby={sys.executable}",
                "--lsp-arg",
                f"ruby={FAKE_LSP}",
                cwd=root,
            )
            self.assertIn("backend=lsp", restored.stdout.splitlines()[0])
            self.assertIn("LspFile", restored.stdout)

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

        for language, relative in (("c", "c_jq/main.c"), ("cpp", "cpp_fmt/format.cc")):
            result = self.run_cli(
                language, "outline", str(REAL_ROOT / relative), expected=3
            )
            self.assertIn("Tree-sitter found", result.stderr)
            self.assertIn("--lsp", result.stderr)

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
        self.assertIn("lines=18-20", result.stdout.splitlines()[0])
        self.assertIn("def fetch", result.stdout)
        self.assertIn("payload =", result.stdout)
        self.assertNotIn("class Client", result.stdout)
        self.assertNotIn("return parse_payload", result.stdout)

        reversed_range = self.run_cli(
            "show", "python_project/package/api.py:20-18", expected=2
        )
        self.assertIn("1 <= START <= END", reversed_range.stderr)
        clamped = self.run_cli("show", "python_project/package/api.py:29-999")
        self.assertIn("lines=29-31", clamped.stdout.splitlines()[0])
        self.assertIn("def résumé", clamped.stdout)
        beyond_start = self.run_cli(
            "show", "python_project/package/api.py:999-1000", expected=2
        )
        self.assertIn("starts at 999", beyond_start.stderr)

    def test_show_line_range_handles_newline_dense_source(self) -> None:
        with tempfile.TemporaryDirectory(prefix="pira-codenav-lines-") as temp:
            root = Path(temp)
            (root / "dense.py").write_text("\n" * 10_000, encoding="utf-8")
            first = self.run_cli("show", "dense.py:1-1", cwd=root)
            self.assertIn("lines=1-1", first.stdout.splitlines()[0])
            result = self.run_cli("show", "dense.py:9999-10000", cwd=root)
            self.assertIn("lines=9999-10000", result.stdout.splitlines()[0])

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

    def test_show_preserves_attached_rust_attributes_and_positions(self) -> None:
        with tempfile.TemporaryDirectory(prefix="pira-codenav-rust-attrs-") as temp:
            root = Path(temp)
            (root / "item.rs").write_text(
                "#[derive(Debug)]\npub struct Item;\n", encoding="utf-8"
            )
            result = self.run_cli("show", "item.rs::Item", cwd=root)
            self.assertIn("range=L1:1-2:17", result.stdout.splitlines()[0])
            self.assertIn("#[derive(Debug)]", result.stdout)

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

            client_selector = next(
                match.group(1)
                for line in outline.stdout.splitlines()
                if line.startswith("class Client ") and (match := SELECTOR_RE.search(line))
            )
            all_stale = self.run_cli(
                "show", selector, client_selector, cwd=root, expected=4
            )
            self.assertIn("failed=2", all_stale.stdout.splitlines()[0])
            self.assertIn("complete=0", all_stale.stdout.splitlines()[0])

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
            self.assertIn("complete=0", header)
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
        self.assertIn(
            "# pira_codenav outline batch files=2 succeeded=1 failed=1 complete=0",
            outline.stdout,
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
        self.assertIn(
            "# pira_codenav imports batch files=2 succeeded=1 failed=1 complete=0",
            imports.stdout,
        )

        failed = self.run_cli(
            "outline",
            "missing.py",
            "also_missing.py",
            expected=2,
        )
        self.assertEqual(2, failed.stdout.count("outline error file="))
        self.assertIn("all outline files failed", failed.stderr)

        imports_failed = self.run_cli(
            "imports", "missing.js", "also_missing.js", expected=2
        )
        self.assertEqual(2, imports_failed.stdout.count("imports error file="))
        self.assertIn("complete=0", imports_failed.stdout)

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
        result = self.run_cli(
            "map", ".", "--max-items", "50", *self.fake_lsp_args()
        )
        self.assertIn("language=python", result.stdout)
        self.assertIn("language=rust", result.stdout)
        self.assertRegex(result.stdout.splitlines()[0], r"\blsp=[1-9]\d*\b")
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
                "files=8",
                "parsed=7",
                "failed=1",
                "unsupported=1",
                "ambiguous=1",
                "shown=3",
                "omitted=4",
                "complete=0",
            ):
                self.assertIn(expected, header)
            self.assertIn('error file="broken.py" code=2', result.stdout)
            shown = [line for line in result.stdout.splitlines()[1:] if line.startswith("file=")]
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

    def test_dependents_reports_files_that_could_not_be_parsed(self) -> None:
        with tempfile.TemporaryDirectory(prefix="pira-codenav-dependent-errors-") as temp:
            root = Path(temp)
            (root / "model.py").write_text("class Model: pass\n", encoding="utf-8")
            (root / "good.py").write_text("from model import Model\n", encoding="utf-8")
            (root / "broken.py").write_bytes(b"\xff")
            result = self.run_cli("dependents", "model.py", cwd=root)
            header = result.stdout.splitlines()[0]
            self.assertIn("scanned=2", header)
            self.assertIn("failed=1", header)
            self.assertIn("complete=0", header)
            self.assertIn("count=1", header)
            self.assertIn("dependent=good.py", result.stdout)

    def test_dependency_commands_fail_when_every_scanned_file_fails(self) -> None:
        with tempfile.TemporaryDirectory(prefix="pira-codenav-all-deps-fail-") as temp:
            root = Path(temp)
            (root / "target.py").write_text("class Target: pass\n", encoding="utf-8")
            (root / "broken.py").write_bytes(b"\xff")

            dependents = self.run_cli(
                "dependents", "target.py", cwd=root, expected=2
            )
            self.assertIn("scanned=1", dependents.stdout.splitlines()[0])
            self.assertIn("failed=1", dependents.stdout.splitlines()[0])
            self.assertIn("complete=0", dependents.stdout.splitlines()[0])
            self.assertIn("all dependents files failed", dependents.stderr)

            (root / "target.py").write_bytes(b"\xff")
            deps = self.run_cli("deps", "target.py", cwd=root, expected=2)
            self.assertIn("files=2", deps.stdout.splitlines()[0])
            self.assertIn("failed=2", deps.stdout.splitlines()[0])
            self.assertIn("complete=0", deps.stdout.splitlines()[0])
            self.assertIn("all deps files failed", deps.stderr)

    def test_dependency_discovery_uses_narrow_cross_language_groups(self) -> None:
        with tempfile.TemporaryDirectory(prefix="pira-codenav-cross-language-") as temp:
            root = Path(temp)
            (root / "target.js").write_text("export const value = 1;\n", encoding="utf-8")
            (root / "consumer.ts").write_text(
                'import { value } from "./target.js";\nconsole.log(value);\n',
                encoding="utf-8",
            )
            result = self.run_cli(
                "javascript", "dependents", "target.js", "--root", ".", cwd=root
            )
            self.assertIn("dependent=consumer.ts", result.stdout)
            self.assertNotIn("failed=", result.stdout.splitlines()[0])

    def test_c_family_dependency_group_includes_all_three_dialects(self) -> None:
        with tempfile.TemporaryDirectory(prefix="pira-codenav-c-family-deps-") as temp:
            root = Path(temp)
            (root / "api.h").write_text("int api(void);\n", encoding="utf-8")
            for name in ("consumer.c", "consumer.cpp", "consumer.cu"):
                (root / name).write_text('#include "api.h"\n', encoding="utf-8")

            result = self.run_cli(
                "cpp", "dependents", "api.h", "--root", ".", cwd=root
            )
            for name in ("consumer.c", "consumer.cpp", "consumer.cu"):
                self.assertIn(f"dependent={name}", result.stdout)
            self.assertNotIn("failed=", result.stdout.splitlines()[0])

    def test_both_direction_dependency_output_is_balanced(self) -> None:
        with tempfile.TemporaryDirectory(prefix="pira-codenav-balanced-deps-") as temp:
            root = Path(temp)
            for name in ("a", "b", "c", "d"):
                (root / f"{name}.py").write_text(f"{name.upper()} = 1\n", encoding="utf-8")
            (root / "target.py").write_text(
                "import a\nimport b\nimport c\nimport d\n", encoding="utf-8"
            )
            (root / "consumer.py").write_text("import target\n", encoding="utf-8")

            result = self.run_cli(
                "deps",
                "target.py",
                "--direction",
                "both",
                "--depth",
                "1",
                "--max-items",
                "2",
                cwd=root,
            )
            edges = [line for line in result.stdout.splitlines() if line.startswith("edge ")]
            self.assertEqual(2, len(edges))
            self.assertTrue(any("direction=import" in line for line in edges))
            self.assertTrue(any("direction=dependent" in line for line in edges))
            self.assertNotIn("failed=", result.stdout.splitlines()[0])

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
        self.assertRegex(python.stdout, r"function\s+command\b")
        self.assertRegex(python.stdout, r"function\s+pass_context\.new_func\b")
        self.assertNotRegex(python.stdout, r"method\s+pass_context\.new_func\b")

        rust = self.run_cli("outline", str(REAL_ROOT / "rust_ripgrep" / "gitignore.rs"))
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

    def test_closed_output_consumer_stops_before_later_lsp_work(self) -> None:
        with tempfile.TemporaryDirectory(prefix="pira-codenav-closed-output-") as temp:
            root = Path(temp)
            (root / "many.py").write_text(
                "".join(f"def item_{index}(): return {index}\n" for index in range(5_000)),
                encoding="utf-8",
            )
            (root / "dirty.py").write_text(
                "def dirty():\n    return (\n", encoding="utf-8"
            )
            starts = root / "starts.log"
            process = subprocess.Popen(
                [
                    str(self.binary),
                    "outline",
                    "many.py",
                    "dirty.py",
                    "--max-items",
                    "5000",
                    *self.fake_lsp_args("--startup-log", str(starts)),
                ],
                cwd=root,
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
            self.assertEqual(0, process.wait(timeout=10), stderr)
            self.assertFalse(starts.exists(), "closed output should prevent later LSP startup")

    def test_unclean_file_requires_lsp_and_lsp_restores_outline_and_show(self) -> None:
        missing = self.run_cli("outline", "malformed.py", expected=3)
        self.assertEqual("", missing.stdout)
        self.assertIn("Tree-sitter found", missing.stderr)
        self.assertIn("rerun with --lsp", missing.stderr)

        outline = self.run_cli(
            "outline", "malformed.py", *self.fake_lsp_args()
        )
        self.assertIn("backend=lsp", outline.stdout.splitlines()[0])
        self.assertRegex(outline.stdout, r"class\s+Incomplete\b")
        self.assertRegex(outline.stdout, r"method\s+Incomplete\.still_visible\b")

        shown = self.run_cli(
            "show", "malformed.py::Incomplete", *self.fake_lsp_args()
        )
        self.assertIn("class Incomplete:", shown.stdout)
        self.assertIn("return 0", shown.stdout)
        self.assertNotIn("broken =", shown.stdout)

    def test_lsp_is_lazy_and_utf16_ranges_return_exact_source(self) -> None:
        with tempfile.TemporaryDirectory(prefix="pira-codenav-lsp-") as temp:
            root = Path(temp)
            clean = root / "clean.py"
            clean.write_text("def native(): return 1\n", encoding="utf-8")
            log = root / "requests.log"
            native = self.run_cli(
                "outline",
                "clean.py",
                *self.fake_lsp_args("--log", str(log)),
                cwd=root,
            )
            self.assertNotIn("backend=", native.stdout.splitlines()[0])
            self.assertFalse(log.exists(), "clean native parsing must not start the LSP")

            dirty = root / "dirty.py"
            dirty.write_text(
                'class UnicodeBox:\n    value = "é😀"\n\n\nbroken = (\n',
                encoding="utf-8",
            )
            shown = self.run_cli(
                "show",
                "dirty.py::UnicodeBox",
                *self.fake_lsp_args("--log", str(log)),
                cwd=root,
            )
            self.assertIn('value = "é😀"', shown.stdout)
            self.assertNotIn("broken =", shown.stdout)
            methods = log.read_text(encoding="utf-8")
            self.assertIn("initialize", methods)
            self.assertIn("textDocument/documentSymbol", methods)

    def test_lsp_configuration_and_capability_failures_are_concise(self) -> None:
        relative = self.run_cli(
            "outline", "malformed.py", "--lsp", "fake-server", expected=2
        )
        self.assertIn("absolute path", relative.stderr)

        missing_server = self.run_cli(
            "outline",
            "malformed.py",
            *self.fake_lsp_args("--disable-symbols"),
            expected=3,
        )
        self.assertIn("document symbols", missing_server.stderr.lower())

        orphan_arg = self.run_cli(
            "outline", "malformed.py", "--lsp-arg", "--stdio", expected=2
        )
        self.assertIn("requires a matching --lsp", orphan_arg.stderr)

    def test_semantic_commands_require_lsp_and_precise_locations(self) -> None:
        with tempfile.TemporaryDirectory(prefix="pira-codenav-semantics-") as temp:
            root = Path(temp)
            source = root / "sample.py"
            source.write_text(
                "class Target: pass\n\n"
                "def first():\n    return Target()\n\n"
                "def second():\n    return Target()\n",
                encoding="utf-8",
            )
            missing = self.run_cli("definition", "sample.py:4:12", cwd=root, expected=2)
            self.assertIn("requires --lsp", missing.stderr)
            imprecise = self.run_cli(
                "definition", "sample.py:4", *self.fake_lsp_args(), cwd=root, expected=2
            )
            self.assertIn("LINE:COLUMN", imprecise.stderr)

            definition = self.run_cli(
                "definition", "sample.py:4:12", *self.fake_lsp_args(), cwd=root
            )
            self.assertIn("count=1", definition.stdout.splitlines()[0])
            self.assertIn("file=sample.py range=L1:7-1:13", definition.stdout)

            references = self.run_cli(
                "references",
                "sample.py:4:12",
                "--max-items",
                "1",
                *self.fake_lsp_args(),
                cwd=root,
            )
            self.assertIn("count=2 shown=1 omitted=1", references.stdout.splitlines()[0])
            self.assertNotIn("range=L1:7-1:13", references.stdout)
            including = self.run_cli(
                "references",
                "sample.py:4:12",
                "--include-declaration",
                *self.fake_lsp_args(),
                cwd=root,
            )
            self.assertIn("count=3", including.stdout.splitlines()[0])
            self.assertIn("range=L1:7-1:13", including.stdout)

            hover = self.run_cli(
                "hover", "sample.py:4:12", *self.fake_lsp_args(), cwd=root
            )
            self.assertIn("format=markdown", hover.stdout.splitlines()[0])
            self.assertIn("begin untrusted LSP hover", hover.stdout)
            self.assertIn("**Target**", hover.stdout)
            self.assertIn("end LSP hover", hover.stdout)

    def test_find_searches_repository_declarations_with_bounded_handoffs(self) -> None:
        with tempfile.TemporaryDirectory(prefix="pira-codenav-find-") as temp:
            root = Path(temp)
            (root / "api.py").write_text(
                "class Widget:\n"
                "    def run(self, value: int) -> int:\n"
                "        return value\n\n"
                "def build_widget():\n"
                "    return Widget()\n",
                encoding="utf-8",
            )
            (root / "other.py").write_text(
                "class Other:\n    def run(self):\n        return None\n",
                encoding="utf-8",
            )

            literal = self.run_cli("find", ".", "widget", cwd=root)
            self.assertIn("mode=literal", literal.stdout.splitlines()[0])
            self.assertIn('name="Widget"', literal.stdout)
            self.assertIn('name="build_widget"', literal.stdout)

            exact = self.run_cli(
                "find", ".", "Widget", "--exact", "--kind", "class", cwd=root
            )
            self.assertIn("matches=1 shown=1", exact.stdout.splitlines()[0])
            self.assertIn('kind=class name="Widget"', exact.stdout)
            self.assertNotIn("build_widget", exact.stdout)

            regex = self.run_cli(
                "find",
                ".",
                r"^Widget\.run$",
                "--regex",
                "--selectors",
                "--signatures",
                cwd=root,
            )
            self.assertIn("mode=regex", regex.stdout.splitlines()[0])
            self.assertIn('name="Widget.run"', regex.stdout)
            self.assertIn("signature=", regex.stdout)
            selector = SELECTOR_RE.search(regex.stdout)
            self.assertIsNotNone(selector)
            shown = self.run_cli("show", selector.group(1), cwd=root)
            self.assertIn("def run(self, value: int) -> int:", shown.stdout)
            self.assertNotIn("def build_widget", shown.stdout)

            bounded = self.run_cli("find", ".", "run", "--max-items", "1", cwd=root)
            self.assertRegex(bounded.stdout.splitlines()[0], r"matches=2 shown=1 omitted=1")

        unicode_name = self.run_cli(
            "python",
            "find",
            "python_project",
            "RÉSUMÉ",
            "--exact",
        )
        self.assertIn('name="résumé"', unicode_name.stdout)

        namespace_suffix = self.run_cli(
            "php",
            "find",
            "php_laravel",
            "COLLECTION",
            "--exact",
            cwd=REAL_ROOT,
        )
        self.assertIn(
            'name="Illuminate\\\\Support\\\\Collection"', namespace_suffix.stdout
        )

    def test_find_batches_independent_queries_in_one_repository_scan(self) -> None:
        with tempfile.TemporaryDirectory(prefix="pira-codenav-find-multi-") as temp:
            root = Path(temp)
            (root / "api.py").write_text(
                "class Widget:\n"
                "    def run(self): return 1\n\n"
                "def build_widget(): return Widget()\n",
                encoding="utf-8",
            )
            result = self.run_cli(
                "find",
                ".",
                "Widget",
                "build_widget",
                "--exact",
                "--selectors",
                "--max-items",
                "1",
                cwd=root,
            )
            lines = result.stdout.splitlines()
            self.assertIn("queries=2", lines[0])
            self.assertIn("matches=2 shown=2", lines[0])
            self.assertIn('query index=1 text="Widget" mode=exact matches=1 shown=1', lines)
            self.assertIn(
                'query index=2 text="build_widget" mode=exact matches=1 shown=1',
                lines,
            )
            self.assertIn('symbol query=1 file=api.py', result.stdout)
            self.assertIn('symbol query=2 file=api.py', result.stdout)
            self.assertEqual(2, len(SELECTOR_RE.findall(result.stdout)))

            oversized = self.run_cli(
                "find",
                ".",
                *[f"query_{index}" for index in range(32)],
                "--max-items",
                "3126",
                cwd=root,
                expected=2,
            )
            self.assertIn("times query count may not exceed 100000", oversized.stderr)

    def test_find_preserves_clean_results_and_uses_lsp_only_for_dirty_files(self) -> None:
        with tempfile.TemporaryDirectory(prefix="pira-codenav-find-partial-") as temp:
            root = Path(temp)
            (root / "clean.py").write_text("def native_item(): return 1\n", encoding="utf-8")
            (root / "dirty.py").write_text("def dirty():\n    return (\n", encoding="utf-8")

            partial = self.run_cli("find", ".", "item", cwd=root)
            header = partial.stdout.splitlines()[0]
            self.assertIn("parsed=1", header)
            self.assertIn("failed=1", header)
            self.assertIn("complete=0", header)
            self.assertIn('name="native_item"', partial.stdout)

            restored = self.run_cli(
                "find", ".", "LspFile", "--exact", *self.fake_lsp_args(), cwd=root
            )
            self.assertIn("files=2", restored.stdout.splitlines()[0])
            self.assertIn("lsp=1", restored.stdout.splitlines()[0])
            self.assertIn("file=dirty.py", restored.stdout)
            self.assertIn("backend=lsp", restored.stdout)

    def test_find_batches_parsing_without_restarting_lsp(self) -> None:
        with tempfile.TemporaryDirectory(prefix="pira-codenav-find-batches-") as temp:
            root = Path(temp)
            for index in range(20):
                (root / f"dirty_{index:02}.py").write_text(
                    f"def item_{index}():\n    return (\n", encoding="utf-8"
                )
            log = root / "requests.log"
            result = self.run_cli(
                "find",
                ".",
                "LspFile",
                "--exact",
                "--max-items",
                "20",
                *self.fake_lsp_args("--log", str(log)),
                cwd=root,
            )
            self.assertIn("files=20", result.stdout.splitlines()[0])
            self.assertIn("lsp=20", result.stdout.splitlines()[0])
            self.assertIn("matches=20 shown=20", result.stdout.splitlines()[0])
            methods = log.read_text(encoding="utf-8").splitlines()
            self.assertEqual(1, methods.count("initialize"))
            self.assertEqual(20, methods.count("textDocument/documentSymbol"))

            oversized = self.run_cli(
                "find", ".", "item", "--max-items", "100001", cwd=root, expected=2
            )
            self.assertIn("may not exceed 100000", oversized.stderr)

    def test_extended_semantics_and_multi_target_reuse(self) -> None:
        with tempfile.TemporaryDirectory(prefix="pira-codenav-extended-semantics-") as temp:
            root = Path(temp)
            source = root / "sample.py"
            source.write_text(
                "class Target: pass\n\n"
                "def first():\n    return Target()\n\n"
                "def second():\n    return Target()\n",
                encoding="utf-8",
            )
            for command in ("implementation", "type-definition"):
                result = self.run_cli(
                    command, "sample.py:4:12", *self.fake_lsp_args(), cwd=root
                )
                self.assertIn(f"pira_codenav {command}", result.stdout.splitlines()[0])
                self.assertIn("file=sample.py range=L1:7-1:13", result.stdout)

            callers = self.run_cli(
                "callers", "sample.py:4:12", *self.fake_lsp_args(), cwd=root
            )
            self.assertIn('name="caller_of_Target"', callers.stdout)
            self.assertIn('callsites="sample.py:L1:7-1:13"', callers.stdout)

            callees = self.run_cli(
                "callees", "sample.py:4:12", *self.fake_lsp_args(), cwd=root
            )
            self.assertIn('name="callee_of_Target"', callees.stdout)
            self.assertIn('callsites="sample.py:L1:7-1:13"', callees.stdout)

            log = root / "requests.log"
            batch = self.run_cli(
                "definition",
                "sample.py:4:12",
                "sample.py:7:12",
                *self.fake_lsp_args("--log", str(log)),
                cwd=root,
            )
            self.assertIn("batch targets=2 succeeded=2", batch.stdout)
            methods = log.read_text(encoding="utf-8").splitlines()
            self.assertEqual(1, methods.count("initialize"))
            self.assertEqual(1, methods.count("textDocument/didOpen"))
            self.assertEqual(2, methods.count("textDocument/definition"))
            self.assertEqual(1, methods.count("textDocument/didClose"))

    def test_lsp_initialization_and_settings_are_bounded_and_forwarded(self) -> None:
        with tempfile.TemporaryDirectory(prefix="pira-codenav-lsp-config-") as temp:
            root = Path(temp)
            (root / "sample.py").write_text(
                "class Target: pass\nTarget()\n", encoding="utf-8"
            )
            initialization = root / "init.json"
            settings = root / "settings.json"
            config_log = root / "config.json"
            initialization.write_text(json.dumps({"mode": "test"}), encoding="utf-8")
            settings.write_text(
                json.dumps({"python": {"analysis": {"level": "strict"}}}),
                encoding="utf-8",
            )
            result = self.run_cli(
                "definition",
                "sample.py:2:1",
                "--lsp-init",
                "init.json",
                "--lsp-settings",
                "settings.json",
                *self.fake_lsp_args(
                    "--config-log",
                    str(config_log),
                    "--request-configuration",
                    "python.analysis",
                ),
                cwd=root,
            )
            self.assertIn("count=1", result.stdout)
            observed = json.loads(config_log.read_text(encoding="utf-8"))
            self.assertEqual({"mode": "test"}, observed["initializationOptions"])
            self.assertEqual(
                {"python": {"analysis": {"level": "strict"}}}, observed["settings"]
            )
            self.assertEqual([{"level": "strict"}], observed["configuration"])

            malformed = root / "malformed.json"
            malformed.write_text("{", encoding="utf-8")
            invalid = self.run_cli(
                "definition",
                "sample.py:2:1",
                "--lsp-init",
                "malformed.json",
                *self.fake_lsp_args(),
                cwd=root,
                expected=2,
            )
            self.assertIn("invalid JSON", invalid.stderr)

            oversized = root / "oversized.json"
            oversized.write_bytes(b'"' + b"x" * (64 * 1024) + b'"')
            too_large = self.run_cli(
                "definition",
                "sample.py:2:1",
                "--lsp-settings",
                "oversized.json",
                *self.fake_lsp_args(),
                cwd=root,
                expected=2,
            )
            self.assertIn("exceeds 64 KiB", too_large.stderr)

    def test_semantic_lsp_position_conversion_and_capabilities(self) -> None:
        with tempfile.TemporaryDirectory(prefix="pira-codenav-semantic-unicode-") as temp:
            root = Path(temp)
            source = root / "unicode.py"
            line = 'value = "😀"; Target()'
            source.write_text("class Target: pass\n" + line + "\n", encoding="utf-8")
            byte_column = len(line[: line.index("Target")].encode()) + 1
            result = self.run_cli(
                "definition",
                f"unicode.py:2:{byte_column}",
                *self.fake_lsp_args(),
                cwd=root,
            )
            self.assertIn("range=L1:7-1:13", result.stdout)

            no_outline_capability = self.run_cli(
                "definition",
                f"unicode.py:2:{byte_column}",
                *self.fake_lsp_args("--disable-symbols"),
                cwd=root,
            )
            self.assertIn("range=L1:7-1:13", no_outline_capability.stdout)

            unsupported = self.run_cli(
                "definition",
                f"unicode.py:2:{byte_column}",
                *self.fake_lsp_args("--disable-semantics"),
                cwd=root,
                expected=3,
            )
            self.assertIn("does not advertise definition", unsupported.stderr)

            duplicate_limit = self.run_cli(
                "definition",
                f"unicode.py:2:{byte_column}",
                "--max-items",
                "1",
                "--max-items",
                "2",
                *self.fake_lsp_args(),
                cwd=root,
                expected=2,
            )
            self.assertIn("only once", duplicate_limit.stderr)

    def test_lsp_server_edit_request_is_refused(self) -> None:
        with tempfile.TemporaryDirectory(prefix="pira-codenav-lsp-edit-") as temp:
            root = Path(temp)
            source = root / "dirty.py"
            source.write_text("class Safe:\n    pass\n\n\nbroken = (\n", encoding="utf-8")
            before = source.read_bytes()
            result = self.run_cli(
                "outline",
                "dirty.py",
                *self.fake_lsp_args("--request-edit"),
                cwd=root,
            )
            self.assertIn("backend=lsp", result.stdout.splitlines()[0])
            self.assertEqual(before, source.read_bytes())
            self.assertNotIn("MUST_NOT_APPEAR", source.read_text(encoding="utf-8"))

    def test_map_reuses_one_lazy_lsp_process(self) -> None:
        with tempfile.TemporaryDirectory(prefix="pira-codenav-lsp-map-") as temp:
            root = Path(temp)
            for index in range(3):
                (root / f"dirty_{index}.py").write_text(
                    f"class Item{index}:\n    pass\n\n\nbroken = (\n",
                    encoding="utf-8",
                )
            log = root / "requests.log"
            result = self.run_cli(
                "map",
                ".",
                *self.fake_lsp_args("--log", str(log)),
                cwd=root,
            )
            self.assertIn("lsp=3", result.stdout.splitlines()[0])
            methods = log.read_text(encoding="utf-8").splitlines()
            self.assertEqual(1, methods.count("initialize"))
            self.assertEqual(3, methods.count("textDocument/documentSymbol"))

    def test_map_preserves_clean_results_when_an_lsp_is_required(self) -> None:
        with tempfile.TemporaryDirectory(prefix="pira-codenav-partial-map-") as temp:
            root = Path(temp)
            (root / "clean.py").write_text("def clean(): return 1\n", encoding="utf-8")
            (root / "dirty.py").write_text("def dirty():\n    return (\n", encoding="utf-8")

            partial = self.run_cli("map", ".", cwd=root)
            header = partial.stdout.splitlines()[0]
            self.assertIn("parsed=1", header)
            self.assertIn("failed=1", header)
            self.assertIn("complete=0", header)
            self.assertIn("file=clean.py", partial.stdout)
            self.assertIn('error file="dirty.py" code=3', partial.stdout)

            (root / "clean.py").unlink()
            failed = self.run_cli("map", ".", cwd=root, expected=3)
            self.assertIn("parsed=0", failed.stdout.splitlines()[0])
            self.assertIn("complete=0", failed.stdout.splitlines()[0])
            self.assertIn('error file="dirty.py" code=3', failed.stdout)

    def test_map_caches_lsp_startup_failure_per_invocation(self) -> None:
        with tempfile.TemporaryDirectory(prefix="pira-codenav-failed-lsp-") as temp:
            root = Path(temp)
            for index in range(3):
                (root / f"dirty_{index}.py").write_text(
                    f"def item_{index}():\n    return (\n", encoding="utf-8"
                )
            starts = root / "starts.log"
            result = self.run_cli(
                "map",
                ".",
                *self.fake_lsp_args(
                    "--startup-log", str(starts), "--exit-on-initialize"
                ),
                cwd=root,
                expected=3,
            )
            self.assertIn("failed=3", result.stdout.splitlines()[0])
            self.assertEqual(["start"], starts.read_text(encoding="utf-8").splitlines())

    def test_show_caches_structural_failure_per_file(self) -> None:
        with tempfile.TemporaryDirectory(prefix="pira-codenav-failed-show-") as temp:
            root = Path(temp)
            (root / "dirty.py").write_text(
                "class Item:\n    pass\n\nbroken = (\n", encoding="utf-8"
            )
            requests = root / "requests.log"
            result = self.run_cli(
                "show",
                "dirty.py:1",
                "dirty.py:2",
                *self.fake_lsp_args("--log", str(requests), "--invalid-range"),
                cwd=root,
                expected=3,
            )
            self.assertIn("failed=2", result.stdout.splitlines()[0])
            methods = requests.read_text(encoding="utf-8").splitlines()
            self.assertEqual(1, methods.count("initialize"))
            self.assertEqual(1, methods.count("textDocument/documentSymbol"))

    def test_mixed_map_uses_per_language_lsp_servers(self) -> None:
        with tempfile.TemporaryDirectory(prefix="pira-codenav-lsp-mixed-") as temp:
            root = Path(temp)
            (root / "dirty.py").write_text(
                "class PythonItem:\n    pass\n\n\nbroken = (\n", encoding="utf-8"
            )
            (root / "dirty.cpp").write_text(
                "#define REGISTER(name) int registered_##name = 0;\n"
                "REGISTER(item)\n",
                encoding="utf-8",
            )
            python_log = root / "python.log"
            cpp_log = root / "cpp.log"
            result = self.run_cli(
                "map",
                ".",
                "--lsp",
                f"python={sys.executable}",
                "--lsp-arg",
                f"python={FAKE_LSP}",
                "--lsp-arg",
                "python=--log",
                "--lsp-arg",
                f"python={python_log}",
                "--lsp",
                f"cpp={sys.executable}",
                "--lsp-arg",
                f"cpp={FAKE_LSP}",
                "--lsp-arg",
                "cpp=--log",
                "--lsp-arg",
                f"cpp={cpp_log}",
                cwd=root,
            )
            self.assertIn("lsp=2", result.stdout.splitlines()[0])
            for log in (python_log, cpp_log):
                methods = log.read_text(encoding="utf-8").splitlines()
                self.assertEqual(1, methods.count("initialize"))
                self.assertEqual(1, methods.count("textDocument/documentSymbol"))

    def test_unknown_language_and_explicit_mismatch_fail_concisely(self) -> None:
        unknown = self.run_cli("outline", "unknown.source", expected=2)
        self.assertIn("cannot determine language", unknown.stderr.lower())
        self.assertIn("pira_codenav LANGUAGE outline", unknown.stderr)

        mismatch = self.run_cli("rust", "outline", "python_project/app.py", expected=2)
        self.assertIn("language mismatch", mismatch.stderr.lower())

    def test_unimplemented_semantic_operations_are_not_commands(self) -> None:
        for command in ("symbols", "workspace-symbol", "calls", "rename"):
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
                ("map", ".", "--max-items", "30", *self.fake_lsp_args()),
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
