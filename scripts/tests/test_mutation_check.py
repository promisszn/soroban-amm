"""Fixture-only tests for scripts/mutation_check.py; no Cargo invocation.

Run with `python3 -m unittest discover -s scripts/tests` (or pytest).
"""

import contextlib
import io
import os
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from mutation_check import (  # noqa: E402
    BaselineError,
    MutationCheck,
    Result,
    find_mutations,
    format_report,
    load_accepted_survivors,
    main,
    production_files,
    production_source,
    run_command,
    select,
)

PASS = "test result: ok. 3 passed; 0 failed"
FAIL = "test result: FAILED. 0 passed; 1 failed"


def completed(returncode, stdout="", stderr=""):
    return subprocess.CompletedProcess(["cargo"], returncode, stdout, stderr)


def runner(*mutation_results, baseline=None):
    """Fake cargo: the first call is the baseline, then one call per mutation."""
    calls = []
    results = [baseline or completed(0, PASS), *mutation_results]

    def run(command, **kwargs):
        calls.append((command, kwargs))
        result = results[min(len(calls), len(results)) - 1]
        if isinstance(result, BaseException) or (
            isinstance(result, type) and issubclass(result, BaseException)
        ):
            raise result
        if callable(result):
            return result(command, **kwargs)
        return result

    run.calls = calls
    return run


class MutationDiscoveryTests(unittest.TestCase):
    def test_discovers_privileged_auth_calls(self):
        source = "admin.require_auth();\ncurrent_admin.require_auth();\n"
        mutations = find_mutations(source, ("auth",))
        self.assertEqual(len(mutations), 2)
        self.assertTrue(all(item.operator == "auth" for item in mutations))

    def test_discovers_keeper_auth_call(self):
        mutations = find_mutations("Self::get_keeper(env)?.require_auth();\n", ("auth",))
        self.assertEqual(len(mutations), 1)

    def test_ignores_user_level_auth(self):
        mutations = find_mutations("caller.require_auth();\nfrom.require_auth();\n", ("auth",))
        self.assertEqual(mutations, [])

    def test_discovers_supported_guard_errors(self):
        source = (
            "return Err(AmmError::DeadlineExceeded);\n"
            "return Err(Error::SlippageExceeded);\n"
            "return Err(Unauthorized);\n"
        )
        self.assertEqual(len(find_mutations(source, ("error",))), 3)

    def test_ignores_unlisted_error_variants(self):
        mutations = find_mutations("return Err(AmmError::NotFound);\n", ("error",))
        self.assertEqual(mutations, [])

    def test_flips_comparison_operators_on_guard_lines(self):
        source = "if deadline < now && min_out >= amount { return Err(E); }\n"
        mutations = find_mutations(source, ("comparison",))
        self.assertEqual([item.mutated for item in mutations], [">=", "<"])
        self.assertTrue(all(item.line == 1 for item in mutations))

    def test_comparison_mutation_is_one_operator_at_a_time(self):
        source = "if expires_at == now { }\n"
        mutations = find_mutations(source, ("comparison",))
        self.assertEqual(len(mutations), 1)
        self.assertEqual(mutations[0].mutated, "!=")

    def test_ignores_comparisons_outside_guard_lines(self):
        self.assertEqual(
            find_mutations("if balance < amount { }\n", ("comparison",)),
            [],
        )

    def test_comparison_ignores_arrows_and_generics(self):
        source = (
            "fn execute_op(env: &Env, deadline: u64) -> Result<i128, ()> {\n"
            "    let fallback: Vec<u64> = deadline_list();\n"
            "    match deadline { 0 => 1, _ => 2 };\n"
            "    let shifted = deadline >> 2;\n"
            "}\n"
        )
        self.assertEqual(find_mutations(source, ("comparison",)), [])

    def test_comparison_still_flips_real_guard_next_to_generics(self):
        source = "if now > order.deadline { return Err::<(), E>(E::X); }\n"
        mutations = find_mutations(source, ("comparison",))
        self.assertEqual([(item.original, item.mutated) for item in mutations], [(">", "<=")])

    def test_admin_prefix_is_not_a_min_guard(self):
        source = (
            "if admin_count > 0 { }\n"
            "if amount < MIN_LIQUIDITY { }\n"
            "if err == DeadlineExceeded { }\n"
        )
        self.assertEqual(find_mutations(source, ("comparison",)), [])

    def test_min_segment_inside_identifier_is_a_guard(self):
        mutations = find_mutations("if out < amount_min_out { }\n", ("comparison",))
        self.assertEqual(len(mutations), 1)

    def test_excludes_inline_test_module(self):
        source = (
            "admin.require_auth();\n"
            "#[cfg(test)]\nmod tests { admin.require_auth(); }\n"
        )
        mutations = find_mutations(source, ("auth",))
        self.assertEqual(len(mutations), 1)

    def test_cfg_test_module_declaration_does_not_truncate_the_file(self):
        source = (
            "#[cfg(test)]\n"
            "mod guard_tests;\n"
            "pub fn first() { admin.require_auth(); }\n"
            "#[cfg(test)]\n"
            "mod tests {\n"
            "    fn helper() { admin.require_auth(); }\n"
            "}\n"
            "pub fn second() { admin.require_auth(); }\n"
        )
        mutations = find_mutations(source, ("auth",))
        self.assertEqual([item.function for item in mutations], ["first", "second"])
        self.assertEqual([item.line for item in mutations], [3, 8])

    def test_excludes_other_cfg_test_items_individually(self):
        source = (
            "#[cfg(test)]\n"
            "use crate::helpers::{a, b};\n"
            "#[cfg(test)]\n"
            "#[allow(dead_code)]\n"
            "fn only_in_tests() -> [u8; 2] { admin.require_auth(); [0; 2] }\n"
            "pub fn production() { admin.require_auth(); }\n"
        )
        mutations = find_mutations(source, ("auth",))
        self.assertEqual([item.function for item in mutations], ["production"])

    def test_cfg_test_inside_strings_and_comments_is_ignored(self):
        source = (
            '// #[cfg(test)]\nconst S: &str = "#[cfg(test)] {";\n'
            "pub fn guarded() { admin.require_auth(); }\n"
        )
        self.assertEqual(len(find_mutations(source, ("auth",))), 1)

    def test_ignores_matches_in_comments_and_strings(self):
        source = (
            "/// admin.require_auth();\n"
            "/* return Err(E::Paused); */\n"
            'const DOC: &str = "admin.require_auth();";\n'
            "// if deadline < now\n"
        )
        self.assertEqual(find_mutations(source), [])

    def test_production_source_preserves_offsets(self):
        source = "#[cfg(test)]\nmod tests { x }\nfn f() {}\n"
        blanked = production_source(source)
        self.assertEqual(len(blanked), len(source))
        self.assertEqual(blanked.index("fn f"), source.index("fn f"))
        self.assertNotIn("mod tests", blanked)


class MutationIdTests(unittest.TestCase):
    SOURCE = (
        "pub fn set_fee() {\n"
        "    admin.require_auth();\n"
        "}\n"
        "pub fn pause() {\n"
        "    admin.require_auth();\n"
        "}\n"
    )

    def ids(self, source):
        return [item.identifier for item in find_mutations(source, ("auth",), crate="amm")]

    def test_ids_survive_edits_above_the_line(self):
        shifted = "// a new comment\nconst X: u32 = 1;\n\n" + self.SOURCE
        self.assertEqual(self.ids(shifted), self.ids(self.SOURCE))

    def test_ids_survive_reindentation(self):
        self.assertEqual(
            self.ids(self.SOURCE.replace("    admin", "        admin")), self.ids(self.SOURCE)
        )

    def test_ids_name_the_function_and_differ_per_function(self):
        ids = self.ids(self.SOURCE)
        self.assertEqual(len(set(ids)), 2)
        self.assertTrue(ids[0].startswith("auth:set_fee:"))
        self.assertTrue(ids[1].startswith("auth:pause:"))

    def test_identical_lines_in_one_function_get_distinct_ids(self):
        source = "pub fn f() {\n    admin.require_auth();\n    admin.require_auth();\n}\n"
        ids = self.ids(source)
        self.assertEqual(len(set(ids)), 2)

    def test_editing_the_mutated_line_changes_its_id(self):
        edited = self.SOURCE.replace(
            "    admin.require_auth();\n}\npub fn pause",
            "    admin . require_auth();\n}\npub fn pause",
        )
        before, after = self.ids(self.SOURCE), self.ids(edited)
        self.assertNotEqual(before[0], after[0])
        self.assertEqual(before[1], after[1])


class ProductionFilesTests(unittest.TestCase):
    def test_follows_production_modules_and_skips_test_modules(self):
        with tempfile.TemporaryDirectory() as directory:
            src = Path(directory)
            (src / "lib.rs").write_text(
                "pub mod math;\n#[cfg(test)]\nmod guard_tests;\nmod tests_helpers;\n"
                "#[cfg(test)]\nmod tests { }\n",
                encoding="utf-8",
            )
            (src / "math.rs").write_text("mod inner;\n", encoding="utf-8")
            (src / "math").mkdir()
            (src / "math" / "inner.rs").write_text("", encoding="utf-8")
            (src / "guard_tests.rs").write_text("", encoding="utf-8")
            (src / "tests_helpers").mkdir()
            (src / "tests_helpers" / "mod.rs").write_text("", encoding="utf-8")
            names = [path.relative_to(src).as_posix() for path in production_files(src)]
            self.assertEqual(
                names, ["lib.rs", "math.rs", "tests_helpers/mod.rs", "math/inner.rs"]
            )


class MutationExecutionTests(unittest.TestCase):
    def setUp(self):
        self.temporary_directory = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary_directory.name)
        self.source_path = self.root / "contracts" / "fixture" / "src" / "lib.rs"
        self.source_path.parent.mkdir(parents=True)
        self.original = "pub fn guard() { admin.require_auth(); }\n"
        self.source_path.write_text(self.original, encoding="utf-8")
        self.config = self.root / "mutations.toml"
        self.config.write_text("[crates.fixture]\n", encoding="utf-8")

    def tearDown(self):
        self.temporary_directory.cleanup()

    def checker(self, command_runner):
        return MutationCheck(
            self.root,
            "fixture",
            operators=("auth",),
            command_runner=command_runner,
        )

    def run_main(self, command_runner, *extra):
        stdout, stderr = io.StringIO(), io.StringIO()
        report = self.root / "report.txt"
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            code = main(
                ["--crate", "fixture", "--config", str(self.config), "--report", str(report), *extra],
                root=self.root,
                command_runner=command_runner,
            )
        return code, stdout.getvalue(), stderr.getvalue(), report

    def assert_restored(self):
        self.assertEqual(self.source_path.read_text(encoding="utf-8"), self.original)

    def test_restores_source_after_killed_mutant(self):
        result = self.checker(runner(completed(101, FAIL))).run()
        self.assertEqual(result[0].status, "killed")
        self.assert_restored()

    def test_restores_source_after_surviving_mutant(self):
        result = self.checker(runner(completed(0, PASS))).run()
        self.assertEqual(result[0].status, "survived")
        self.assert_restored()

    def test_restores_source_after_timeout(self):
        result = self.checker(runner(subprocess.TimeoutExpired(["cargo"], 1))).run()
        self.assertEqual(result[0].status, "timed out")
        self.assert_restored()

    def test_restores_source_after_keyboard_interrupt(self):
        with self.assertRaises(KeyboardInterrupt):
            self.checker(runner(KeyboardInterrupt)).run()
        self.assert_restored()

    def test_mutated_source_is_what_the_tests_see(self):
        seen = []

        def capture(command, **kwargs):
            seen.append(self.source_path.read_text(encoding="utf-8"))
            return completed(101, FAIL)

        self.checker(runner(capture)).run()
        self.assertEqual(seen, ["pub fn guard() {  }\n"])
        self.assert_restored()

    def test_skips_compile_error_and_restores_source(self):
        result = self.checker(
            runner(completed(101, "", "error[E0308]: mismatched types"))
        ).run()
        self.assertEqual(result[0].status, "skipped (compile error)")
        self.assert_restored()

    def test_unclassified_command_failure_surfaces_and_restores_source(self):
        with self.assertRaisesRegex(RuntimeError, "without reporting"):
            self.checker(runner(completed(101, "", "failed to download dependency"))).run()
        self.assert_restored()

    def test_runs_baseline_then_cargo_for_the_requested_package(self):
        fake = runner(completed(101, FAIL))
        self.checker(fake).run()
        expected = ["cargo", "test", "-p", "fixture", "--lib"]
        self.assertEqual([command for command, _ in fake.calls], [expected, expected])
        self.assertEqual(fake.calls[0][1]["timeout"], 300)

    def test_baseline_test_failure_raises_before_any_mutation(self):
        fake = runner(baseline=completed(101, FAIL))
        with self.assertRaisesRegex(BaselineError, "failed on the unmutated source"):
            self.checker(fake).run()
        self.assertEqual(len(fake.calls), 1)
        self.assert_restored()

    def test_baseline_compile_error_exits_nonzero(self):
        fake = runner(baseline=completed(101, "", "error[E0425]: cannot find value `x`"))
        code, stdout, stderr, report = self.run_main(fake)
        self.assertEqual(code, 2)
        self.assertIn("baseline", stderr)
        self.assertIn("E0425", stderr)
        self.assertNotIn("Unaccepted survivors: 0", stdout)
        self.assertIn("ERROR: baseline", report.read_text(encoding="utf-8"))
        self.assertEqual(len(fake.calls), 1)

    def test_baseline_timeout_exits_nonzero(self):
        code, _, stderr, _ = self.run_main(runner(baseline=subprocess.TimeoutExpired(["cargo"], 1)))
        self.assertEqual(code, 2)
        self.assertIn("timed out", stderr)

    def test_main_passes_when_every_mutant_is_killed(self):
        code, stdout, _, _ = self.run_main(runner(completed(101, FAIL)))
        self.assertEqual(code, 0)
        self.assertIn("Unaccepted survivors: 0", stdout)

    def test_main_fails_on_unaccepted_survivor(self):
        code, stdout, _, _ = self.run_main(runner(completed(0, PASS)))
        self.assertEqual(code, 1)
        self.assertIn("UNACCEPTED survived: contracts/fixture/src/lib.rs:1", stdout)

    def test_all_compile_errors_mean_no_viable_mutations(self):
        code, stdout, _, _ = self.run_main(runner(completed(101, "", "error[E0308]: x")))
        self.assertEqual(code, 2)
        self.assertIn("compile_errors=1", stdout)
        self.assertIn("no viable mutation", stdout)

    def test_no_discovered_mutations_is_an_error(self):
        self.source_path.write_text("pub fn nothing() {}\n", encoding="utf-8")
        fake = runner()
        code, _, stderr, _ = self.run_main(fake)
        self.assertEqual(code, 2)
        self.assertIn("no mutations found", stderr)
        self.assertEqual(fake.calls, [])

    def test_list_mode_does_not_run_cargo(self):
        fake = runner()
        code, stdout, _, _ = self.run_main(fake, "--list")
        self.assertEqual(code, 0)
        self.assertIn("contracts/fixture/src/lib.rs:1\tauth:guard:", stdout)
        self.assertEqual(fake.calls, [])

    def test_mutates_and_restores_submodule_files(self):
        (self.source_path.parent / "lib.rs").write_text("pub mod admin_ops;\n", encoding="utf-8")
        module = self.source_path.parent / "admin_ops.rs"
        module.write_text(self.original, encoding="utf-8")
        seen = []

        def capture(command, **kwargs):
            seen.append(module.read_text(encoding="utf-8"))
            return completed(101, FAIL)

        results = self.checker(runner(capture)).run()
        self.assertEqual(results[0].mutation.path, "contracts/fixture/src/admin_ops.rs")
        self.assertEqual(seen, ["pub fn guard() {  }\n"])
        self.assertEqual(module.read_text(encoding="utf-8"), self.original)


class RunCommandTests(unittest.TestCase):
    @unittest.skipUnless(Path("/proc").is_dir() and hasattr(os, "killpg"), "needs Linux /proc")
    def test_timeout_kills_the_whole_process_group(self):
        script = (
            "import subprocess, sys, time\n"
            "child = subprocess.Popen(['sleep', '60'])\n"
            "print(child.pid, flush=True)\n"
            "time.sleep(60)\n"
        )
        with self.assertRaises(subprocess.TimeoutExpired) as raised:
            run_command([sys.executable, "-c", script], cwd=None, timeout=2)
        grandchild = int(raised.exception.output.split()[0])

        def alive(pid):
            # The process can exit between open() and read(); on Linux that
            # read raises ProcessLookupError rather than FileNotFoundError.
            try:
                status = Path(f"/proc/{pid}/status").read_text()
            except (FileNotFoundError, ProcessLookupError):
                return False
            return "\nState:\tZ" not in status

        # SIGKILL delivery is asynchronous, so give the kernel a moment to
        # reap the grandchild. A process group that was never killed keeps
        # `sleep 60` running well past this deadline, so the check still fails.
        deadline = time.monotonic() + 5
        while alive(grandchild) and time.monotonic() < deadline:
            time.sleep(0.05)
        self.assertFalse(alive(grandchild))

    def test_returns_completed_process(self):
        result = run_command([sys.executable, "-c", "print('ok')"], cwd=None, timeout=30)
        self.assertEqual((result.returncode, result.stdout.strip()), (0, "ok"))


class SurvivorRatchetTests(unittest.TestCase):
    def test_accepted_survivor_is_reported_but_not_unaccepted(self):
        mutation = find_mutations("admin.require_auth();\n", ("auth",))[0]
        report, unaccepted, viable = format_report(
            "fixture",
            [Result(mutation, "survived")],
            {mutation.identifier: "fixture exemption"},
        )
        self.assertEqual((unaccepted, viable), (0, 1))
        self.assertIn("ACCEPTED survived", report)

    def test_unlisted_survivor_prints_location_and_text(self):
        mutation = find_mutations(
            "admin.require_auth();\n", ("auth",), path="contracts/fixture/src/lib.rs"
        )[0]
        report, unaccepted, _ = format_report("fixture", [Result(mutation, "survived")], {})
        self.assertEqual(unaccepted, 1)
        self.assertIn("contracts/fixture/src/lib.rs:1 [auth;", report)
        self.assertIn("original: admin.require_auth();", report)
        self.assertIn("mutated:  (removed)", report)

    def test_report_counts_compile_errors(self):
        mutations = find_mutations("admin.require_auth();\nkeeper.require_auth();\n", ("auth",))
        report, _, viable = format_report(
            "fixture",
            [Result(mutations[0], "killed"), Result(mutations[1], "skipped (compile error)")],
            {},
        )
        self.assertEqual(viable, 1)
        self.assertIn("Mutations: run=2, viable=1, compile_errors=1", report)

    def test_stale_accepted_entries_are_reported(self):
        mutations = find_mutations("admin.require_auth();\n", ("auth",))
        report, _, _ = format_report("fixture", [], {"auth:gone:0000000000": "old"}, mutations)
        self.assertIn("STALE accepted survivor (matches no current mutation): auth:gone:", report)

    def test_loads_accepted_survivor_justification(self):
        with tempfile.TemporaryDirectory() as directory:
            config = Path(directory) / "mutations.toml"
            config.write_text(
                "[crates.fixture]\n"
                'accepted_survivors = [{ id = "auth:guard:0123456789", '
                'justification = "known nested authorization" }]\n',
                encoding="utf-8",
            )
            self.assertEqual(
                load_accepted_survivors(config, "fixture"),
                {"auth:guard:0123456789": "known nested authorization"},
            )

    def test_empty_accepted_list_has_no_exemptions(self):
        with tempfile.TemporaryDirectory() as directory:
            config = Path(directory) / "mutations.toml"
            config.write_text("[crates.fixture]\naccepted_survivors = []\n", encoding="utf-8")
            self.assertEqual(load_accepted_survivors(config, "fixture"), {})

    def test_blank_justification_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            config = Path(directory) / "mutations.toml"
            config.write_text(
                '[[crates.fixture.accepted_survivors]]\nid = "x"\njustification = " "\n',
                encoding="utf-8",
            )
            with self.assertRaisesRegex(ValueError, "no justification"):
                load_accepted_survivors(config, "fixture")


class SelectionTests(unittest.TestCase):
    def test_shards_partition_the_mutations_and_limit_caps_them(self):
        items = list(range(7))
        shards = [select(items, (index, 3)) for index in (1, 2, 3)]
        self.assertEqual(shards, [[0, 3, 6], [1, 4], [2, 5]])
        self.assertEqual(sorted(sum(shards, [])), items)
        self.assertEqual(select(items, None, 2), [0, 1])


if __name__ == "__main__":
    unittest.main()
