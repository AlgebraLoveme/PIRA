"""Offline tests: synthetic published assets and mocked authenticated GitHub IO."""
from __future__ import annotations

import copy
import gzip
import hashlib
import io
import json
import os
import shutil
import subprocess
import textwrap
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from tools.build import package_github_release as package, release_plan
from tools.build import test_package_github_release as bundle_fixtures


class MixedReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.repository = "owner/repository"
        self.source = "a" * 40
        self.origins = {}
        self.blobs = {}
        self.next_id = 100
        self.runs = {}
        index = {"schema_version": 2, "repository": self.repository, "tag": "pira-tools-original",
                 "source_sha": self.source, "tools": {}}
        files = {}
        for tool in package.TOOLS:
            binaries = {}
            for platform in package.PLATFORMS:
                suffix = ".exe" if platform.startswith("windows-") else ""
                name = f"{tool}-1.0.0-{platform}{suffix}.gz"
                raw = f"{tool}:{platform}:original".encode()
                data = gzip.compress(raw, mtime=123)  # Recompression must not substitute for copying.
                files[name] = data
                binaries[platform] = {"asset": name, "compression": "gzip", "sha256": self.sha(raw),
                                      "size": len(raw), "asset_sha256": self.sha(data), "asset_size": len(data)}
            index["tools"][tool] = {"version": "1.0.0", "binaries": binaries}
        self.origin = self.add_origin(index, files)
        self.tag = index["tag"]
        for target, replacement in (("github_json", self.api), ("github_asset", self.download)):
            mock = patch.object(package, target, side_effect=replacement)
            mock.start()
            self.addCleanup(mock.stop)

    @staticmethod
    def sha(data):
        return hashlib.sha256(data).hexdigest()

    def add_origin(self, index, files):
        self.next_id += 100
        release = {"id": self.next_id, "tag_name": index["tag"], "target_commitish": index["source_sha"],
                   "draft": False, "prerelease": False, "assets": []}
        for name, data in files.items():
            self.next_id += 1
            asset = {"id": self.next_id, "name": name, "size": len(data),
                     "digest": "sha256:" + self.sha(data), "state": "uploaded"}
            release["assets"].append(asset)
            self.blobs[asset["id"]] = data
        self.next_id += 1
        release["assets"].append({"id": self.next_id, "name": "pira-tools-release.json", "state": "uploaded"})
        origin = {"release": release, "index": index}
        self.origins[index["tag"]] = origin
        self.refresh(origin)
        return origin

    def refresh(self, origin=None):
        origin = origin or self.origin
        asset = next(a for a in origin["release"]["assets"] if a["name"] == "pira-tools-release.json")
        data = json.dumps(origin["index"]).encode()
        self.blobs[asset["id"]] = data
        asset.update(size=len(data), digest="sha256:" + self.sha(data))

    def api(self, repository, path):
        self.assertEqual(repository, self.repository)
        if path.startswith("releases/tags/"):
            return copy.deepcopy(self.origins[path.removeprefix("releases/tags/")]["release"])
        if path.startswith("git/ref/tags/"):
            origin = self.origins[path.removeprefix("git/ref/tags/")]
            return {"object": {"type": "commit", "sha": origin["index"]["source_sha"]}}
        if path.startswith("actions/runs/"):
            return self.runs[int(path.rsplit("/", 1)[1])]
        self.fail(f"unexpected GitHub request: {path}")

    def download(self, repository, asset_id, limit):
        self.assertEqual(repository, self.repository)
        return self.blobs[asset_id]

    def requests(self, tools=None):
        return {tool: {"tag": self.tag, "version": "1.0.0", "source_sha": self.source}
                for tool in (tools if tools is not None else package.TOOLS[2:])}

    def receipts(self, tools=None):
        return package.resolve_retained(self.repository, self.requests(tools))

    def assemble(self, receipts, name="release", expected_versions=None):
        artifacts = self.root / (name + "-artifacts")
        artifacts.mkdir()
        for tool in package.TOOLS:
            if tool not in receipts:
                bundle_fixtures.PackageGitHubReleaseTests().make_bundle(artifacts, tool, "2.0.0")
        output = self.root / name
        path = package.package_release(artifacts, output, repository=self.repository, tag="pira-tools-mixed",
                                       source_sha="b" * 40, retained=receipts, build_run_id=900,
                                       expected_versions=expected_versions)
        return json.loads(path.read_text()), output

    def test_explicit_four_retained_tools_preserve_all_twenty_gzip_assets_and_actual_origins(self):
        receipts = self.receipts()
        index, output = self.assemble(receipts)
        self.assertEqual(index["source_sha_scope"], "assembly")
        self.assertEqual(index["source_sha"], "b" * 40)
        self.assertEqual(len(list(output.iterdir())), 31)
        for tool, record in index["tools"].items():
            if tool in receipts:
                self.assertEqual(record["version"], "1.0.0")
                self.assertEqual(record["build_source_sha"], self.source)
                self.assertEqual(record["retained_from"]["release_id"], self.origin["release"]["id"])
                for platform, binary in record["binaries"].items():
                    asset = receipts[tool]["assets"][platform]
                    self.assertEqual(binary["origin_asset_id"], asset["id"])
                    self.assertEqual((output / binary["asset"]).read_bytes(), self.blobs[asset["id"]])
            else:
                self.assertEqual(record["build_source_sha"], "b" * 40)
                self.assertEqual(record["build_run_id"], 900)
                self.assertNotIn("retained_from", record)

    def test_team_first_then_ctx_dec_keeps_three_pinned_tools_and_complete_bundle(self):
        retained = ["pira_nav", "pira_svg_check", "pira_write_check"]
        for tool in retained:
            manifest = self.root / f"tools/crates/{tool}/Cargo.toml"
            manifest.parent.mkdir(parents=True)
            manifest.write_text('[package]\nversion="1.0.0"\n')
        args = ["release_plan", "--root", str(self.root), "--repository", self.repository,
                "--retained-tools", json.dumps(self.requests(retained)), "--first-tools", '["pira_team"]']
        with patch("sys.argv", args), patch("builtins.print") as printed:
            release_plan.main()
        plan = json.loads(printed.call_args.args[0])
        self.assertEqual(plan["changed"], ["pira_team"])
        self.assertEqual(plan["remaining"], ["pira_ctx", "pira_dec"])
        self.assertEqual(set(plan["build_tools"]), {"pira_team", "pira_ctx", "pira_dec"})
        self.assertTrue(plan["has_changed"] and plan["has_remaining"])
        index, output = self.assemble(plan["retained_tools"])
        self.assertEqual(len(list(output.iterdir())), 31)
        self.assertEqual(index["tools"]["pira_team"]["build_source_sha"], "b" * 40)
        self.assertNotIn("retained_from", index["tools"]["pira_team"])
        for tool in retained:
            for platform, binary in index["tools"][tool]["binaries"].items():
                asset = plan["retained_tools"][tool]["assets"][platform]
                self.assertEqual((output / binary["asset"]).read_bytes(), self.blobs[asset["id"]])
        with patch("sys.argv", args[:-1] + ['["pira_nav"]']), patch("sys.stderr", new_callable=io.StringIO):
            with self.assertRaises(SystemExit) as error:
                release_plan.main()
            self.assertEqual(error.exception.code, 2)

    def test_first_tools_rejects_unknown_duplicate_or_nonlist_values(self):
        for text in ('null', '{}', '[{}]', '["unknown"]', '["pira_team", "pira_team"]'):
            with self.subTest(text=text), self.assertRaises(package.PackageError):
                release_plan.first_tools(text)
        self.assertEqual(release_plan.first_tools('[]'), [])

    def test_existing_schema2_reader_accepts_additive_origins_without_changing_binary_selection(self):
        from assets.scripts.test_setup_pira_tools import setup
        index, output = self.assemble(self.receipts())
        with patch.object(setup, "RELEASE_REPOSITORY", self.repository), \
             patch.object(setup, "request_bytes", return_value=json.dumps(index).encode()):
            loaded = setup.release_index()
        self.assertEqual(loaded, index)
        for tool in package.TOOLS:
            selected = setup.tool_selection(loaded, tool, "linux-x64", self.root / "not-installed")
            self.assertEqual(selected.version, index["tools"][tool]["version"])
            self.assertEqual(selected.asset_hash, self.sha((output / selected.asset).read_bytes()))

    def test_pins_reject_unknown_tools_invalid_fields_duplicate_keys_and_unchanged_input_claims(self):
        good = self.requests([package.TOOLS[-1]])
        for value in ([], {"unknown": next(iter(good.values()))},
                      {package.TOOLS[-1]: {**next(iter(good.values())), "unchanged_inputs": True}},
                      {package.TOOLS[-1]: {"tag": self.tag, "version": "1.0.0", "source_sha": "master"}},
                      self.requests(package.TOOLS)):
            with self.subTest(value=value), self.assertRaises(package.PackageError):
                package.retained_requests(json.dumps(value))
        with self.assertRaisesRegex(package.PackageError, "duplicate"):
            package.retained_requests('{"pira_team":{},"pira_team":{}}')

    def test_explicit_retention_never_claims_current_inputs_are_unchanged(self):
        for tool in package.TOOLS[2:]:
            manifest = self.root / f"tools/crates/{tool}/Cargo.toml"
            manifest.parent.mkdir(parents=True)
            manifest.write_text('[package]\nversion="1.0.0"\n')
        plan = release_plan.plan_retained(self.root, self.repository, self.requests())
        self.assertEqual(plan["changed"], list(package.TOOLS[:2]))
        self.assertEqual(plan["build_tools"], list(package.TOOLS[:2]))
        self.assertFalse(plan["has_remaining"])
        self.assertIn("no current-input equivalence claimed", plan["reuse"])
        tool = package.TOOLS[-1]
        (self.root / f"tools/crates/{tool}/Cargo.toml").write_text('[package]\nversion="9.0.0"\n')
        with self.assertRaisesRegex(package.PackageError, "assembly manifest version"):
            release_plan.plan_retained(self.root, self.repository, self.requests())

    def test_version_and_actual_build_source_pins_must_match_authenticated_origin(self):
        for field, value in (("version", "9.0.0"), ("source_sha", "c" * 40)):
            requests = self.requests([package.TOOLS[-1]])
            requests[package.TOOLS[-1]][field] = value
            with self.subTest(field=field), self.assertRaisesRegex(package.PackageError, "pin disagreement"):
                package.resolve_retained(self.repository, requests)

    def test_malformed_source_version_platform_and_metadata_are_rejected(self):
        original = copy.deepcopy(self.origin)
        tool = package.TOOLS[-1]
        platform = package.PLATFORMS[-1]
        cases = [
            lambda o: o["release"].update(draft=True),
            lambda o: o["release"].update(prerelease=True),
            lambda o: o["index"].update(repository="other/repository"),
            lambda o: o["index"].update(schema_version=2.0),
            lambda o: o["index"].update(source_sha="c" * 40),
            lambda o: o["index"]["tools"][tool]["binaries"].pop(platform),
            lambda o: o["index"]["tools"][tool].update(version="wrong"),
            lambda o: o["index"]["tools"][tool]["binaries"][platform].update(asset_sha256="f" * 64),
            lambda o: o["release"]["assets"].append(copy.deepcopy(o["release"]["assets"][0])),
            lambda o: o["index"].update(source_sha_scope="unknown"),
            lambda o: o["index"]["tools"][tool].update(build_source_sha="c" * 40),
            lambda o: o["index"].update(source_sha_scope="assembly"),
        ]
        for mutate in cases:
            with self.subTest(case=cases.index(mutate)):
                self.origin = copy.deepcopy(original)
                self.origins[self.tag] = self.origin
                mutate(self.origin)
                self.refresh()
                with self.assertRaises(package.PackageError):
                    self.receipts([tool])

    def test_git_tag_and_index_digest_disagreements_are_rejected(self):
        with patch.object(package, "github_json", side_effect=lambda repo, path:
                          {"object": {"type": "commit", "sha": "f" * 40}} if path.startswith("git/") else self.api(repo, path)):
            with self.assertRaisesRegex(package.PackageError, "Git tag/source"):
                self.receipts()
        asset = self.origin["release"]["assets"][-1]
        self.blobs[asset["id"]] += b' '
        with self.assertRaisesRegex(package.PackageError, "index digest/size"):
            self.receipts()

    def test_changed_receipt_or_fresh_bundle_for_retained_tool_never_publishes_an_index(self):
        receipts = self.receipts()
        receipts[package.TOOLS[-1]]["release_id"] += 1
        with self.assertRaisesRegex(package.PackageError, "changed since planning"):
            self.assemble(receipts)
        receipts = self.receipts()
        artifacts = self.root / "duplicate"
        artifacts.mkdir()
        bundle_fixtures.PackageGitHubReleaseTests().make_bundle(artifacts, package.TOOLS[-1], "1.0.0")
        with self.assertRaisesRegex(package.PackageError, "also has a fresh bundle"):
            package.package_release(artifacts, self.root / "no-index", repository=self.repository,
                                    tag="pira-tools-test", source_sha="b" * 40, retained=receipts)
        self.assertFalse((self.root / "no-index/pira-tools-release.json").exists())

    def test_compressed_download_and_raw_binary_integrity_fail_independently(self):
        receipts = self.receipts()
        target = receipts[package.TOOLS[-1]]["assets"][package.PLATFORMS[-1]]
        self.blobs[target["id"]] += b'corrupt'
        with self.assertRaisesRegex(package.PackageError, "compressed asset digest/size"):
            self.assemble(receipts)
        # Publish a consistently rehashed gzip containing the wrong raw executable.
        data = gzip.compress(b'wrong executable')
        tool = package.TOOLS[-1]
        binary = self.origin["index"]["tools"][tool]["binaries"][package.PLATFORMS[-1]]
        binary.update(asset_sha256=self.sha(data), asset_size=len(data))
        api_asset = next(a for a in self.origin["release"]["assets"] if a["id"] == target["id"])
        api_asset.update(size=len(data), digest='sha256:' + self.sha(data))
        self.blobs[target["id"]] = data
        self.refresh()
        with self.assertRaisesRegex(package.PackageError, "raw binary digest/size"):
            self.assemble(self.receipts(), "bad-raw")

    def test_packaged_versions_must_equal_the_assembly_manifests(self):
        with self.assertRaisesRegex(package.PackageError, "assembly source manifests"):
            self.assemble(self.receipts(), expected_versions={t: "9.0.0" for t in package.TOOLS})
        self.assertFalse((self.root / "release/pira-tools-release.json").exists())

    def test_mixed_origin_can_be_retained_again_with_verified_transitive_identity(self):
        index, output = self.assemble(self.receipts())
        self.runs[900] = {"head_sha": "b" * 40, "status": "completed", "conclusion": "success"}
        self.add_origin(index, {p.name: p.read_bytes() for p in output.iterdir() if p.name != 'pira-tools-release.json'})
        requests = self.requests([package.TOOLS[-1]])
        requests[package.TOOLS[-1]]["tag"] = index["tag"]
        receipts = package.resolve_retained(self.repository, requests)
        self.assertEqual(receipts[package.TOOLS[-1]]["record"]["build_source_sha"], self.source)
        index["tools"][package.TOOLS[-1]]["retained_from"]["index_sha256"] = "f" * 64
        self.refresh(self.origins[index["tag"]])
        with self.assertRaisesRegex(package.PackageError, "provenance source/version/identity"):
            package.resolve_retained(self.repository, requests)

    def test_malformed_git_ref_objects_fail_as_release_errors(self):
        for value in (None, [], "bad", {"type": "tag", "sha": "bad"}):
            with self.subTest(value=value), patch.object(package, "github_json", side_effect=lambda repo, path:
                    {"object": value} if path.startswith("git/") else self.api(repo, path)):
                with self.assertRaises(package.PackageError):
                    self.receipts()

    def test_only_full_source_commit_ids_can_be_packaged(self):
        for source in ("b" * 41, "b" * 63, None):
            with self.subTest(source=source), self.assertRaisesRegex(package.PackageError, "invalid source commit SHA"):
                package.package_release(self.root, self.root / "invalid", repository=self.repository,
                                        tag="pira-tools-invalid", source_sha=source)

    def test_explicit_null_receipt_file_is_not_treated_as_full_fresh_mode(self):
        receipt_file = self.root / "receipts.json"
        receipt_file.write_text("null")
        args = ["package_github_release", "--artifacts-dir", str(self.root),
                "--output-dir", str(self.root / "output"), "--repository", self.repository,
                "--tag", "pira-tools-test", "--source-sha", "b" * 40,
                "--retained-manifest", str(receipt_file)]
        with patch("sys.argv", args), self.assertRaisesRegex(package.PackageError, "JSON receipt object"):
            package.main()

    def test_build_run_and_source_must_agree(self):
        tool = package.TOOLS[-1]
        self.origin["index"]["tools"][tool]["build_run_id"] = 900
        self.runs[900] = {"head_sha": self.source, "status": "completed", "conclusion": "success"}
        self.refresh()
        self.receipts([tool])
        for key, value in (("head_sha", "f" * 40), ("status", "in_progress"), ("conclusion", "failure")):
            with self.subTest(key=key), patch.dict(self.runs[900], {key: value}):
                with self.assertRaisesRegex(package.PackageError, "build run/source"):
                    self.receipts([tool])

    def test_github_raw_output_uses_only_flags_supported_by_the_existing_cli(self):
        for help_text, expected in (("FLAGS --allow-escape-sequences", ["--allow-escape-sequences"]), ("FLAGS", [])):
            package.github_raw_flags.cache_clear()
            with self.subTest(help_text=help_text), patch.object(package.subprocess, "check_output", return_value=help_text):
                self.assertEqual(package.github_raw_flags(), expected)
        package.github_raw_flags.cache_clear()

    @unittest.skipUnless(shutil.which("pwsh"), "native PowerShell unavailable")
    def test_windows_workflow_runs_only_fresh_packages_or_full_default_and_rejects_empty_selection(self):
        workflow = Path(__file__).resolve().parents[2] / ".github/workflows/build-pira-tool-bundles.yml"
        section = workflow.read_text().split("  test-windows:\n", 1)[1].split("\n  publish:\n", 1)[0]
        block = textwrap.dedent(section[section.index("          if ($env:RETAINED_TOOLS"):])
        prefix = "$ErrorActionPreference = 'Stop'\nfunction cargo { $script:called = @($args); $global:LASTEXITCODE = 0 }\n"
        suffix = "\nConvertTo-Json -InputObject @($script:called) -Compress\n"
        common = ["test", "--manifest-path", "tools/Cargo.toml"]
        for retained, tools, expected in (
            ({}, list(package.TOOLS), common + ["--workspace", "--locked", "--", "--test-threads=1"]),
            ({"selected": True}, list(package.TOOLS[:2]), common + ["--locked", "--package", package.TOOLS[0], "--package", package.TOOLS[1], "--", "--test-threads=1"]),
        ):
            with self.subTest(retained=retained):
                env = dict(os.environ, RETAINED_TOOLS=json.dumps(retained, separators=(",", ":")), BUILD_TOOLS=json.dumps(tools))
                result = subprocess.run([shutil.which("pwsh"), "-NoProfile", "-NonInteractive", "-Command", prefix + block + suffix],
                                        env=env, capture_output=True, text=True, timeout=20)
                self.assertEqual(result.returncode, 0, result.stderr)
                # PowerShell functions consume the native end-of-options marker.
                self.assertIn(" -- --test-threads=1", block)
                self.assertEqual(json.loads(result.stdout), [arg for arg in expected if arg != "--"])
        env = dict(os.environ, RETAINED_TOOLS='{"selected":true}', BUILD_TOOLS="[]")
        result = subprocess.run([shutil.which("pwsh"), "-NoProfile", "-NonInteractive", "-Command", prefix + block + suffix],
                                env=env, capture_output=True, text=True, timeout=20)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("No fresh tool packages selected", result.stderr)

    def test_provenance_ids_must_be_integers_not_equal_valued_json_floats(self):
        index, output = self.assemble(self.receipts())
        origin = self.add_origin(index, {p.name: p.read_bytes() for p in output.iterdir() if p.name != 'pira-tools-release.json'})
        tool = package.TOOLS[-1]
        requests = self.requests([tool])
        requests[tool]["tag"] = index["tag"]
        for field in ("release_id", "index_asset_id"):
            previous = index["tools"][tool]["retained_from"]
            original = previous[field]
            previous[field] = float(original)
            self.refresh(origin)
            with self.subTest(field=field), self.assertRaises(package.PackageError):
                package.resolve_retained(self.repository, requests)
            previous[field] = original
        record = index["tools"][tool]["binaries"][package.PLATFORMS[-1]]
        record["origin_asset_id"] = float(record["origin_asset_id"])
        self.refresh(origin)
        with self.assertRaises(package.PackageError):
            package.resolve_retained(self.repository, requests)

    def test_nonstandard_json_constants_and_cyclic_origins_are_rejected(self):
        for text in ('{"unexpected":NaN}', '{"unexpected":Infinity}'):
            with self.subTest(text=text), self.assertRaises(package.PackageError):
                package.strict_json(text)
        self.origin["index"]["source_sha_scope"] = "assembly"
        for record in self.origin["index"]["tools"].values():
            record["build_source_sha"] = self.source
        self.origin["index"]["tools"][package.TOOLS[-1]]["retained_from"] = {
            "release_id": self.origin["release"]["id"], "tag": self.tag,
            "assembly_source_sha": self.source, "index_asset_id": self.origin["release"]["assets"][-1]["id"],
            "index_sha256": "a" * 64}
        self.refresh()
        with self.assertRaisesRegex(package.PackageError, "cyclic/overlong"):
            self.receipts([package.TOOLS[-1]])

    def test_null_retained_provenance_cannot_claim_a_different_build_source(self):
        self.origin["index"]["source_sha_scope"] = "assembly"
        for record in self.origin["index"]["tools"].values():
            record["build_source_sha"] = self.source
        tool = package.TOOLS[-1]
        self.origin["index"]["tools"][tool].update(build_source_sha="f" * 40, retained_from=None)
        self.refresh()
        requests = self.requests([tool])
        requests[tool]["source_sha"] = "f" * 40
        with self.assertRaisesRegex(package.PackageError, "malformed retained provenance"):
            package.resolve_retained(self.repository, requests)

    def test_malformed_retained_provenance_is_not_a_fresh_source_claim(self):
        self.origin["index"]["source_sha_scope"] = "assembly"
        for record in self.origin["index"]["tools"].values():
            record["build_source_sha"] = self.source
        tool = package.TOOLS[-1]
        self.origin["index"]["tools"][tool]["retained_from"] = {"tag": self.tag}
        self.refresh()
        with self.assertRaisesRegex(package.PackageError, "malformed retained provenance"):
            self.receipts([tool])


if __name__ == "__main__":
    unittest.main()
