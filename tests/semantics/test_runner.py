#!/usr/bin/env python3
"""Driver contract tests; no compiler/session is started here."""
import argparse
import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

module = importlib.util.spec_from_file_location("semantics_runner", Path(__file__).with_name("run.py"))
runner = importlib.util.module_from_spec(module)
module.loader.exec_module(runner)


class AdapterContract(unittest.TestCase):
    def exercise(self, path="live", output=None, identity=None, timeout=False, environment=False, base_output=None, case_name="01-test"):
        spec = {
            "case": case_name, "v0_1": ["works"], "state_rule": "counter retained",
            "base_probes": ["probe=11", "probe=12"],
            "steps": [{"name": "edit-1", "stock_after_restart": ["probe=20", "probe=30"],
                       "live_expected": ["probe=22", "probe=32"]}],
        }
        if environment:
            spec["steps"][0]["environment"] = {"M4_SEMANTICS_REQUIRED": "22"}
            path = "session-relaunch"
        if output is None:
            output = spec["steps"][0]["live_expected" if path == "live" else "stock_after_restart"]
        observed = iter((base_output if base_output is not None else spec["base_probes"]) + output)
        identities = iter([(100, "1"), identity or ((100, "1") if path == "live" else (101, "2")),
                           identity or ((100, "1") if path == "live" else (101, "2"))])
        class Session:
            def __init__(self, command, source, env, *args):
                if "-v" in command or "--verbose" in command:
                    raise AssertionError("controller config must not pollute app stdout")
                if "CARGO_TARGET_DIR" in env or "RUSTD_HOME" in env:
                    raise AssertionError("unsupported caller environment reached installer")
            def wait_event(self, start, predicate, label):
                if label.endswith("ready"):
                    return {"event": "ready"}
                if timeout:
                    raise RuntimeError("no save event")
                return {"event": "save", "path": path, "reason": "test controller reason"}
            def events(self):
                return []
            def probe(self):
                return next(observed)
            def identity(self):
                return next(identities)
            def close(self):
                pass
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            case = root / spec["case"]
            case.mkdir()
            (case / "edits.toml").write_text('''schema=1
case="01-test"
v0_1=["works"]
state_rule="counter retained"
base_probes=["probe=11", "probe=12"]
[[steps]]
name="edit-1"
stock_after_restart=["probe=20", "probe=30"]
live_expected=["probe=22", "probe=32"]
''')
            contract = case / "edits.toml"
            contract.write_text(contract.read_text().replace('case="01-test"', f'case="{case_name}"'))
            if environment:
                with (case / "edits.toml").open("a") as contract:
                    contract.write('[steps.environment]\nM4_SEMANTICS_REQUIRED="22"\n')
            (case / "expected.json").write_text(json.dumps(runner.expected_contract(spec)))
            args = argparse.Namespace(lane=root / "no-lane", controller=None, support=None,
                                      archive=None, timeout=1, offline=True, default_profile=False)
            with patch.object(runner, "RefreshApp", Session), contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                return runner.run_refresh(case, root / "work", args)

    def test_launch_scrubs_caller_overrides_and_keeps_stdout_clean(self):
        with patch.dict(os.environ, {"CARGO_TARGET_DIR": "/caller-target", "RUSTD_HOME": "/caller-rustd"}):
            self.assertEqual(self.exercise()["result"], "PASS")

    def test_profile_variant_preserves_optimized_inline_case(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for optimized in (False, True):
                case, copy = root / str(optimized), root / (str(optimized) + "-copy")
                case.mkdir()
                copy.mkdir()
                manifest = '[package]\nname="profile-test"\n[profile.dev]\ndebug = 0\n'
                if optimized:
                    manifest += 'opt-level = 1\n'
                (case / "Cargo.toml").write_text(manifest)
                (copy / "Cargo.toml").write_text(manifest)
                normalized = runner.qualified_default_profile(case, copy)
                self.assertEqual(normalized, [] if optimized else ["Cargo.toml"])
                self.assertEqual((case / "Cargo.toml").read_text(), manifest)
                if optimized:
                    self.assertEqual((copy / "Cargo.toml").read_text(), manifest)
                else:
                    self.assertNotIn("profile.dev", (copy / "Cargo.toml").read_text())

    def test_wrong_base_remains_failure_but_does_not_hide_edit(self):
        result = self.exercise(base_output=["probe=wrong", "probe=wrong"])
        self.assertEqual(result["result"], "FAIL")
        self.assertEqual(result["base-verdict"], "FAIL")
        self.assertEqual(result["steps"][0]["output-verdict"], "PASS")

    def test_live_requires_retained_output(self):
        self.assertEqual(self.exercise()["result"], "PASS")
        wrong = self.exercise(output=["probe=20", "probe=30"])
        self.assertEqual(wrong["result"], "FAIL")
        self.assertEqual(wrong["steps"][0]["observed-output"], ["probe=20", "probe=30"])

    def test_safe_fallback_is_separate_from_path_expectation(self):
        result = self.exercise(path="stock")
        self.assertEqual(result["result"], "PASS")
        self.assertFalse(result["path-match"])
        self.assertEqual(result["steps"][0]["reason"], "test controller reason")

    def test_unannounced_restart_fails_even_with_correct_live_text(self):
        self.assertEqual(self.exercise(identity=(101, "2"))["result"], "FAIL")

    def test_restart_event_requires_new_process(self):
        self.assertEqual(self.exercise(path="restart", identity=(100, "1"))["result"], "FAIL")

    def test_missing_event_cannot_pass_from_probe_alone(self):
        result = self.exercise(timeout=True)
        self.assertEqual(result["result"], "FAIL")
        self.assertEqual(result["steps"][0]["actual-path"], "no-result")
        self.assertEqual(result["steps"][0]["diagnostic-probe"], "probe=22")

    def test_environment_relaunch_does_not_claim_stock_save_coverage(self):
        result = self.exercise(environment=True)
        self.assertEqual(result["result"], "PARTIAL")
        self.assertFalse(result["coverage-complete"])
        self.assertEqual(result["steps"][0]["actual-path"], "session-relaunch")
        self.assertEqual(result["steps"][0]["output-verdict"], "PASS")

    def test_stock_only_build_script_does_not_require_a_compiler_reset(self):
        result = self.exercise(path="stock", case_name="16-build-output")
        self.assertEqual(result["result"], "PASS")
        self.assertEqual(result["steps"][0]["recapture-verdict"],
                         "NOT-APPLICABLE: compiler disabled by installation policy")

    def test_error_outcome_is_not_treated_as_restart(self):
        result = self.exercise(path="error")
        self.assertEqual(result["result"], "FAIL")
        self.assertIn("undelivered edit", result["steps"][0]["error"])


if __name__ == "__main__":
    unittest.main()
