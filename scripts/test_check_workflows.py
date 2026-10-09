#!/usr/bin/env python3
"""Tests of tools/check-workflows.py: the real workflows pass, and each rule
fails on a small workflow that breaks it (so the check cannot rot into
accepting everything). Run: python3 -I tools/test_check_workflows.py"""

import importlib.util
import os
import sys
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("check_workflows", os.path.join(HERE, "check-workflows.py"))
cw = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cw)

SHA = "3d3c42e5aac5ba805825da76410c181273ba90b1"
GOOD = f"""name: t
on:
  push:
    branches: [main]
  pull_request:
permissions:
  contents: read
jobs:
  a:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@{SHA} # v7.0.1
        with:
          persist-credentials: false
      - name: build
        env:
          REF: ${{{{ github.ref_name }}}}
        run: |
          echo "$REF" "${{{{ github.workspace }}}}"
"""


def check(text, rel=".github/workflows/t.yml"):
    errors = []
    cw.check_file(rel, rel, text, errors)
    return errors


class Rules(unittest.TestCase):
    def test_real_workflows_pass(self):
        root = os.path.join(HERE, "..")
        files = cw.workflow_files(root)
        self.assertGreaterEqual(len(files), 3)
        errors = []
        for rel in files:
            with open(os.path.join(root, rel), encoding="utf-8") as f:
                cw.check_file(rel, rel, f.read(), errors)
        self.assertEqual(errors, [])

    def test_good_passes(self):
        self.assertEqual(check(GOOD), [])

    def bad(self, text, needle, rel=".github/workflows/t.yml"):
        errors = check(text, rel)
        self.assertTrue(any(needle in e for e in errors), f"{needle!r} not in {errors}")

    def test_pull_request_target(self):
        self.bad(GOOD.replace("  pull_request:\n", "  pull_request_target:\n"), "pull_request_target")

    def test_workflow_run(self):
        self.bad(GOOD.replace("  pull_request:\n", "  workflow_run:\n    workflows: [x]\n"), "workflow_run")

    def test_no_permissions(self):
        self.bad(GOOD.replace("permissions:\n  contents: read\n", ""), "no top-level `permissions:`")

    def test_top_level_write(self):
        self.bad(GOOD.replace("contents: read", "contents: write", 1), "top level is read-only")

    def test_write_all(self):
        self.bad(GOOD.replace("permissions:\n  contents: read", "permissions: write-all"), "write-all")

    def test_job_write_not_listed(self):
        self.bad(GOOD.replace("    runs-on:", "    permissions:\n      contents: write\n    runs-on:"), "write permission")

    def test_unpinned_action(self):
        self.bad(GOOD.replace(f"@{SHA} # v7.0.1", "@v7"), "not pinned to a full commit sha")

    def test_pinned_without_version_comment(self):
        self.bad(GOOD.replace(" # v7.0.1", ""), "no `# vX.Y.Z` comment")

    def test_docker_action(self):
        self.bad(GOOD.replace(f"actions/checkout@{SHA} # v7.0.1", "docker://alpine:3"), "docker://")

    def test_checkout_keeps_credentials(self):
        self.bad(GOOD.replace("persist-credentials: false", "fetch-depth: 0"), "persist-credentials")

    def test_expression_in_run(self):
        self.bad(GOOD.replace('echo "$REF"', 'echo "${{ github.head_ref }}"'), "inside a run script")

    def test_input_in_run(self):
        self.bad(GOOD.replace('echo "$REF"', 'echo "${{ inputs.x }}"'), "inside a run script")

    def test_step_output_in_run(self):
        self.bad(GOOD.replace('echo "$REF"', 'echo "${{ steps.a.outputs.b }}"'), "inside a run script")

    def test_secret_without_environment(self):
        self.bad(GOOD.replace("REF: ${{ github.ref_name }}", "REF: ${{ secrets.TOKEN }}"), "no `environment:`")

    def test_secret_in_run(self):
        t = GOOD.replace("    runs-on: ubuntu-latest\n", "    runs-on: ubuntu-latest\n    environment: release\n")
        self.bad(t.replace('echo "$REF"', 'echo "${{ secrets.TOKEN }}"'), "secrets.TOKEN")

    def test_secret_with_environment_ok(self):
        t = GOOD.replace("    runs-on: ubuntu-latest\n", "    runs-on: ubuntu-latest\n    environment: release\n")
        self.assertEqual(check(t.replace("REF: ${{ github.ref_name }}", "REF: ${{ secrets.TOKEN }}")), [])

    def test_secrets_inherit(self):
        t = GOOD + "  b:\n    uses: ./.github/workflows/x.yml\n    secrets: inherit\n"
        self.bad(t, "secrets: inherit")

    def test_curl_pipe_shell(self):
        self.bad(GOOD.replace('echo "$REF"', "curl -fsSL https://x.example/i | sudo bash"), "pipes a download into a shell")

    def test_cache_saved_on_pull_request(self):
        t = GOOD + f"      - uses: actions/cache/save@55cc8345863c7cc4c66a329aec7e433d2d1c52a9 # v6.1.0\n        with:\n          path: x\n          key: k\n"
        self.bad(t, "cache is saved on a pull request")

    def test_cache_saved_on_main_ok(self):
        t = GOOD + (
            "      - uses: actions/cache/save@55cc8345863c7cc4c66a329aec7e433d2d1c52a9 # v6.1.0\n"
            "        if: ${{ success() && github.event_name == 'push' && github.ref == 'refs/heads/main' }}\n"
            "        with:\n          path: x\n          key: k\n"
        )
        self.assertEqual(check(t), [])

    def test_unpinned_container(self):
        self.bad(GOOD.replace("    runs-on:", "    container: alpine:3\n    runs-on:"), "not pinned by digest")

    def test_template_placeholders_only_in_template(self):
        t = GOOD.replace(f"actions/checkout@{SHA} # v7.0.1", "o/r/.github/workflows/w.yml@<FRAMEWORK_SHA> # vX.Y.Z")
        self.assertEqual(check(t, "template/.github/workflows/t.yml"), [])
        self.bad(t, "not pinned to a full commit sha")

    def test_reader_refuses_what_it_does_not_understand(self):
        self.bad("name: t\non: [push]\nx: &anchor\n  a: b\n", "cannot read this workflow safely")
        self.bad("name: t\n\ton: push\n", "cannot read this workflow safely")


class Reader(unittest.TestCase):
    def test_subset(self):
        doc = cw.parse_yaml(
            "a: 1  # c\nb:\n  - x\n  - y: 2\n    z: 3\nc: |\n  line1\n    line2\n\n  line3\nd: ['v*', w]\ne:\n  f: [main]\n"
        )
        self.assertEqual(doc["a"], "1")
        self.assertEqual(doc["b"], ["x", {"y": "2", "z": "3"}])
        self.assertEqual(doc["c"], "line1\n  line2\n\nline3")
        self.assertEqual(doc["d"], ["v*", "w"])
        self.assertEqual(doc["e"], {"f": ["main"]})

    def test_duplicate_key(self):
        with self.assertRaises(cw.YamlError):
            cw.parse_yaml("a: 1\na: 2\n")


if __name__ == "__main__":
    unittest.main()
