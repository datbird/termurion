#!/usr/bin/env python3
"""Tests for the m14r rename tools (table, dry run, apply). No GUI, no network.

Run: python3 tools/test_rename_table.py   (or python3 -m unittest tools/test_rename_table.py)
"""
import importlib.util
import os
import shutil
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(HERE)
TABLE = os.path.join(HERE, "rename_table.toml")


def load(name):
    path = os.path.join(HERE, name + ".py")
    spec = importlib.util.spec_from_file_location(name, path)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def git(root, *args):
    return subprocess.run(["git", "-C", root, *args], check=True, capture_output=True, text=True).stdout


def slurp(path, mode="r"):
    with open(path, mode) as f:
        return f.read()


_TEMPS = []


def tearDownModule():
    for d in _TEMPS:
        shutil.rmtree(d, ignore_errors=True)


def make_repo(files):
    """A temporary git repo holding the given {path: bytes} files, committed."""
    root = tempfile.mkdtemp(prefix="rename-fixture-")
    _TEMPS.append(root)
    git(root, "init", "-q")
    git(root, "config", "user.email", "t@example.invalid")
    git(root, "config", "user.name", "t")
    for rel, data in files.items():
        full = os.path.join(root, rel)
        os.makedirs(os.path.dirname(full), exist_ok=True)
        with open(full, "wb") as f:
            f.write(data)
    git(root, "add", "-A")
    git(root, "commit", "-q", "-m", "fixture")
    return root


def synthetic_path(scope):
    """A concrete path that matches a scope glob."""
    return scope.replace("**", "x").replace("*", "x")


class TableTests(unittest.TestCase):
    def setUp(self):
        self.core = load("rename_core")
        self.table = self.core.load_table(TABLE)

    def test_rename_table_is_exact(self):
        rows = self.table.rows
        self.assertGreater(len(rows), 40)
        seen = set()
        for r in rows:
            self.assertIn(r.mode, ("exact", "word", "keep"), r)
            self.assertTrue(isinstance(r.old, str) and r.old != "", "empty old: %r" % (r,))
            self.assertFalse(hasattr(r, "regex"))
            self.assertIn(r.group, list("ABCDEF") + ["K", "T"], r)
            self.assertTrue(r.reason.strip(), "row without reason: %r" % (r.old,))
            key = (r.old, tuple(r.scope))
            self.assertNotIn(key, seen, "duplicate row %r" % (key,))
            seen.add(key)
            if r.mode != "keep":
                self.assertIn("tandem", r.old.lower(), "a rename row must hit the old name: %r" % (r.old,))
                self.assertTrue(r.new and "tandem" not in r.new.lower(), "bad new for %r" % (r.old,))
            else:
                self.assertEqual(r.new, r.old, "a keep row maps to itself: %r" % (r.old,))
        # Every keep row wins over every shorter or equal rename row that sits inside it.
        for k in [r for r in rows if r.mode == "keep"]:
            path = synthetic_path(k.scope[0])
            self.assertTrue(self.core.glob_match(k.scope[0], path), (k.scope, path))
            sel = self.core.resolve(self.table, path, k.old)
            covered = [False] * len(k.old)
            for s in sel:
                if s.row.mode == "keep":
                    for i in range(s.start, s.end):
                        covered[i] = True
            self.assertTrue(all(covered), "keep row %r does not win over a rename row" % (k.old,))
        # Keep rows the plan demands.
        keep_old = {r.old for r in rows if r.mode == "keep"}
        for needed in [
            "tandem workgroup approval key\\0v1",
            "tandem autofill contact card\\0v1",
            "{6A1E8D3C-2B7F-4C9E-9D41-5F0B7A2C8E13}",
            "/mnt/c/Users/datbi/src/tandem",
            "in tandem",
            "com.googlecode.iterm2",
            ".itermtab",
            "iTerm.app",
            "LICENSE-iTerm2",
            "COPYING-iTerm2",
            "OSC 1337",
            "showAbout:",
        ]:
            self.assertIn(needed, keep_old)
        paths = {(p.glob, p.mode) for p in self.table.paths}
        self.assertIn(("docs/superpowers/plans/**", "keep-history"), paths)
        self.assertIn(("docs/superpowers/rulings/**", "keep-history"), paths)
        self.assertIn(("tests/fixtures/tmux/*.bin", "keep"), paths)

    def test_new_art_rows_recorded(self):
        news = {r.new for r in self.table.rows if r.group == "F"}
        for name in [
            "docs/assets/termurion/termurion-mark.svg",
            "docs/assets/termurion/termurion-mark.png",
            "docs/assets/termurion/termurion-wordmark.svg",
            "docs/assets/termurion/termurion-wordmark.png",
            "docs/assets/termurion/termurion-social.svg",
            "docs/assets/termurion/termurion-social.png",
            "docs/assets/termurion/termurion-icon-16.png",
            "docs/assets/termurion/termurion-icon-32.png",
            "docs/assets/termurion/termurion-icon-48.png",
            "docs/assets/termurion/termurion-icon-256.png",
            "docs/assets/termurion/termurion.ico",
        ]:
            self.assertIn(name, {a.path for a in self.table.assets}, name)
        self.assertTrue(any(a.old == "docs/assets/tandem-logo.svg" for a in self.table.assets))
        self.assertTrue(news is not None)

    def test_keep_wins_ties_and_longest_wins(self):
        core = self.core
        t = core.Table(
            rows=[
                core.Row("B", "tandem", "termurion", "exact", ["**"], "r"),
                core.Row("K", "in tandem", "in tandem", "keep", ["**"], "r"),
                core.Row("B", "tandem.exe", "termurion.exe", "exact", ["**"], "r"),
            ],
            paths=[],
            assets=[],
        )
        sel = core.resolve(t, "src/a.rs", "run in tandem with tandem.exe")
        got = [(s.row.old, s.row.mode) for s in sel]
        self.assertEqual(got, [("in tandem", "keep"), ("tandem.exe", "exact")])


class DryrunTests(unittest.TestCase):
    def test_dryrun_lists_all_hits(self):
        files = {
            "src/a.rs": b'fn x() { "Tandem" } // tandem tandem\nlet y = 1;\n',
            "README.md": b"Tandem is run in tandem.\nTANDEM_UI and a TanDem oddity\n",
            "docs/superpowers/plans/p.md": b"Tandem plan, tandem again\n",
            "tests/fixtures/tmux/f.bin": b"%output \\033 /mnt/c/Users/datbi/src/tandem\n",
            "src/none.rs": b"nothing here\n",
        }
        root = make_repo(files)
        dry = load("rename_dryrun")
        rep = dry.scan(root, TABLE)
        grep_lines = sum(
            int(line.rsplit(":", 1)[1])
            for line in subprocess.run(
                ["git", "-C", root, "grep", "-ci", "tandem"], capture_output=True, text=True
            ).stdout.splitlines()
        )
        grep_hits = len(
            subprocess.run(["git", "-C", root, "grep", "-ioh", "tandem"], capture_output=True, text=True)
            .stdout.splitlines()
        )
        self.assertEqual(rep.total_lines, grep_lines)
        self.assertEqual(rep.total_hits, grep_hits)
        # The odd-case hit has no exact-case row and is the only UNMAPPED one outside keep paths.
        self.assertEqual(rep.unmapped_outside_history, 1)

    def test_dryrun_writes_nothing(self):
        root = make_repo({"src/a.rs": b"Tandem\n"})
        before = git(root, "status", "--porcelain")
        load("rename_dryrun").scan(root, TABLE)
        self.assertEqual(git(root, "status", "--porcelain"), before)


class ApplyTests(unittest.TestCase):
    def tiny_table(self, root):
        path = os.path.join(root, "tiny.toml")
        with open(path, "w") as f:
            f.write(
                '[[row]]\ngroup="A"\nold="Tandem"\nnew="Termurion"\nmode="exact"\nscope=["**"]\nreason="x"\n'
                '[[row]]\ngroup="B"\nold="tandem.exe"\nnew="termurion.exe"\nmode="exact"\nscope=["**"]\nreason="x"\n'
                '[[row]]\ngroup="K"\nold="in tandem"\nnew="in tandem"\nmode="keep"\nscope=["**"]\nreason="x"\n'
            )
        return path

    def run_apply(self, root, table, *args):
        return subprocess.run(
            [sys.executable, os.path.join(HERE, "rename_apply.py"), "--root", root, "--table", table, *args],
            capture_output=True,
            text=True,
        )

    def test_apply_refuses_unmapped(self):
        root = make_repo(
            {"a.txt": b"Tandem and tandem.exe, odd tandem here\n", "b.txt": b"Tandem\n"}
        )
        table = self.tiny_table(root)  # untracked helper, outside the file list
        before_a = slurp(os.path.join(root, "a.txt"), "rb")
        before_b = slurp(os.path.join(root, "b.txt"), "rb")
        res = self.run_apply(root, table, "a.txt", "b.txt")
        self.assertNotEqual(res.returncode, 0, res.stdout)
        self.assertIn("UNMAPPED", res.stdout + res.stderr)
        # Nothing changed, not even the file with only mapped hits.
        self.assertEqual(slurp(os.path.join(root, "a.txt"), "rb"), before_a)
        self.assertEqual(slurp(os.path.join(root, "b.txt"), "rb"), before_b)

    def test_apply_changes_only_mapped_rows(self):
        root = make_repo({"a.txt": b"Tandem runs tandem.exe in tandem now\n"})
        table = self.tiny_table(root)
        out = os.path.join(root, "out.diff")
        res = self.run_apply(root, table, "--diff", out, "a.txt")
        self.assertEqual(res.returncode, 0, res.stdout + res.stderr)
        self.assertEqual(
            slurp(os.path.join(root, "a.txt")), "Termurion runs termurion.exe in tandem now\n"
        )
        self.assertIn("termurion.exe", slurp(out))

    def test_apply_group_filter_and_dry(self):
        root = make_repo({"a.txt": b"Tandem runs tandem.exe\n"})
        table = self.tiny_table(root)
        res = self.run_apply(root, table, "--groups", "B", "a.txt")
        self.assertEqual(res.returncode, 0, res.stdout + res.stderr)
        self.assertEqual(slurp(os.path.join(root, "a.txt")), "Tandem runs termurion.exe\n")
        res = self.run_apply(root, table, "--check", "a.txt")
        self.assertEqual(res.returncode, 0)


if __name__ == "__main__":
    unittest.main()
