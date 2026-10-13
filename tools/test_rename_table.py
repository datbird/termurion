#!/usr/bin/env python3
"""Tests for the m14r rename tools (table, dry run, apply). No GUI, no network.

Run: python3 tools/test_rename_table.py   (or python3 -m unittest tools/test_rename_table.py)
"""
import importlib.util
import os
import re
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
            self.assertIn(r.mode, ("exact", "word", "keep", "keep-word", "prose"), r)
            self.assertTrue(isinstance(r.old, str) and r.old != "", "empty old: %r" % (r,))
            self.assertFalse(hasattr(r, "regex"))
            self.assertIn(r.group, list("ABCDEF") + ["K", "T"], r)
            self.assertTrue(r.reason.strip(), "row without reason: %r" % (r.old,))
            key = (r.old, tuple(r.scope))
            self.assertNotIn(key, seen, "duplicate row %r" % (key,))
            seen.add(key)
            if not r.mode.startswith("keep"):
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
        keep_old = {r.old for r in rows if r.mode.startswith("keep")}
        for needed in [
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
        # (Bare rows exist only for prose and file-level; a code file with no row of its own is
        # UNMAPPED hit for hit: 3 in src/a.rs, plus the odd-case TanDem in the README.)
        self.assertEqual(rep.unmapped_outside_history, 4)

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


STORAGE_KEYS = {
    "src/arrangement.rs": [
        "Tandem Contents",
        "Tandem Session Reference",
        "Tandem Adopt Reference",
        "Tandem Stable Session ID",
        "Tandem Adopt Stable Session ID",
        "Tandem Styled Contents",
        "Tandem Banner",
        "Tandem Cursor",
        "Tandem Attach",
        "Tandem Idle",
    ],
    "src/desktops.rs": ["Tandem Virtual Desktop"],
    "src/workgroup_restore.rs": ["Tandem Attach"],
}

BARE = {"Tandem", "tandem", "TANDEM"}
PROSE_SCOPES = {"docs/**", "README.md", "progress.md"}


class ClassificationTests(unittest.TestCase):
    def setUp(self):
        self.core = load("rename_core")
        self.table = self.core.load_table(TABLE)

    def test_storage_keys_resolve_to_keep(self):
        for path, keys in STORAGE_KEYS.items():
            for key in keys:
                text = 'pub const K: &str = "%s";' % key
                start = text.index(key)
                sel = self.core.resolve(self.table, path, text)
                owner = [s for s in sel if s.start <= start and start + len(key) <= s.end]
                self.assertEqual(len(owner), 1, (path, key))
                self.assertEqual(owner[0].row.mode, "exact", (path, key))  # scope change: renamed, not kept
                self.assertEqual((owner[0].start, owner[0].end), (start, start + len(key)), (path, key))

    def test_no_catchall_bare_rows_in_code(self):
        for r in self.table.rows:
            if r.old in BARE and r.mode == "prose":
                self.assertTrue(set(r.scope) <= PROSE_SCOPES, "bare prose row with wide scope: %r" % (r.scope,))
            elif r.old in BARE:
                # A bare row in code is allowed only as a file-level row: one concrete file, no glob.
                self.assertEqual(r.mode, "exact", r)
                self.assertEqual(len(r.scope), 1, "bare row scoped to several files: %r" % (r.scope,))
                self.assertNotRegex(r.scope[0], r"[*?]", "bare row with a glob scope: %r" % (r.scope,))
                self.assertTrue(r.scope[0].startswith(("src/", "schema/", "resources/", "scripts/", "tests/", "crates/", "tools/")), r.scope)
            if r.mode == "prose":
                self.assertIn(r.old, BARE)

    def test_no_prefix_rows(self):
        for r in self.table.rows:
            # A prefix literal may stay only as a file-level row for a string that really is that
            # exact literal (e.g. a test asserting the absence of "__tandem"); never with a glob.
            if r.old in ("tandem-", "tandem_", "_tandem", "TANDEM_", "__tandem", "Tandem-", "Tandem_"):
                self.assertNotRegex("".join(r.scope), r"[*?]", "prefix row with a glob scope: %r" % (r.old,))
            if r.mode in ("exact", "word") and r.group in "BCD" and re.search(r"[*?]", "".join(r.scope)):
                self.assertNotRegex(r.old, r"^(?i:tandems?)[-_]$|^_+(?i:tandems?)$", "prefix row %r" % (r.old,))

    def test_env_vars_and_pipes_are_enumerated_aliases(self):
        alias_old = {r.old for r in self.table.rows if r.alias}
        for needed in [
            "TANDEM_UI", "TANDEM_SCRIPT", "TANDEM_SSH_TRANSPORT", "TANDEM_PAGE", "TANDEM_T", "TANDEM_WAIT_PID",
            "{name} API Key for Tandem", "Tandem Terminal Handoff", "Tandem/", "Tandem-Browser/", '"Tandem-"',
            "pipe\\tandem-host-", "pipe\\tandem-url-", "pipe\\tandem-ssh-secrets-",
        ]:
            self.assertIn(needed, alias_old, needed)
        for r in self.table.rows:
            if r.alias:
                self.assertTrue(r.group in "BCADTF" and r.mode == "exact", r)
                self.assertNotIn(r.old, BARE)

    def test_alias_scopes_are_narrow(self):
        for r in self.table.rows:
            if r.alias:
                self.assertNotIn("**", r.scope, "alias row with global scope: %r" % (r.old,))

    def test_keep_word_is_word_bounded(self):
        core = self.core
        t = core.Table(
            rows=[
                core.Row("B", "tandem", "termurion", "exact", ["**"], "r"),
                core.Row("K", "in tandem", "in tandem", "keep-word", ["**"], "r"),
            ]
        )
        sel = core.resolve(t, "src/a.rs", "run in tandem now; main tandem; in tandems; (in tandem)")
        kept = [(s.start, s.end) for s in sel if s.row.mode == "keep-word"]
        text = "run in tandem now; main tandem; in tandems; (in tandem)"
        self.assertEqual([text[a:b] for a, b in kept], ["in tandem", "in tandem"])
        self.assertEqual(kept[0][0], 4)
        self.assertEqual(kept[1][0], text.rindex("in tandem"))

    def test_table_has_word_bounded_in_tandem(self):
        modes = {(r.old, r.mode) for r in self.table.rows if r.old.lower() == "in tandem"}
        self.assertEqual(modes, {("in tandem", "keep-word"), ("In tandem", "keep-word")})
        sel = self.core.resolve(self.table, "src/a.rs", "main tandem and In tandem")
        self.assertEqual([s.row.old for s in sel if s.row.mode.startswith("keep")], ["In tandem"])

    def test_tracked_files_skips_backup_tree(self):
        root = make_repo({"src/a.rs": b"x\n", ".rename-backup/src/a.rs": b"Tandem\n"})
        self.assertEqual(self.core.tracked_files(root), ["src/a.rs"])

    def test_tandems_test_names_are_a_decided_kind(self):
        # "tandems_..." is the plural of the product name in test names: Tandem's / copies of Tandem.
        rows = [r for r in self.table.rows if r.old.startswith("tandems_")]
        self.assertTrue(rows)
        for r in rows:
            self.assertEqual(r.new, "termurions_" + r.old[len("tandems_"):])


class DryrunReportTests(unittest.TestCase):
    def test_alias_and_prose_sections(self):
        root = make_repo(
            {
                "src/a.rs": b'const P: &str = "tandem-host";\n',
                "README.md": b"Tandem is a terminal.\n",
            }
        )
        dry = load("rename_dryrun")
        core = load("rename_core")
        tdir = tempfile.mkdtemp(prefix="rename-table-")
        _TEMPS.append(tdir)
        tp = os.path.join(tdir, "t.toml")
        with open(tp, "w") as f:
            f.write(
                "[[row]]\ngroup='B'\nold='tandem-host'\nnew='termurion-host'\nmode='exact'\nalias=true\n"
                "scope=['src/a.rs']\nreason='x'\n"
                "[[row]]\ngroup='E'\nold='Tandem'\nnew='Termurion'\nmode='prose'\nscope=['README.md']\nreason='x'\n"
            )
        rep = dry.scan(root, tp)
        out = dry.render(rep, tp)
        self.assertIn("ALIAS rows", out)
        self.assertIn("src/a.rs:1", out.split("ALIAS rows", 1)[1])
        self.assertIn("PROSE rows", out)
        self.assertIn("README.md (1)", out.split("PROSE rows", 1)[1])
        self.assertEqual(rep.unmapped_outside_history, 0)

    def test_real_table_has_zero_unmapped(self):
        dry = load("rename_dryrun")
        rep = dry.scan(REPO, TABLE)
        self.assertEqual(rep.unmapped_outside_history, 0, rep.unmapped[:5])
        self.assertTrue(rep.by_alias)


class ApplySafetyTests(unittest.TestCase):
    def run_apply(self, root, table, *args):
        return subprocess.run(
            [sys.executable, os.path.join(HERE, "rename_apply.py"), "--root", root, "--table", table, *args],
            capture_output=True,
            text=True,
        )

    def test_keep_only_notice(self):
        root = make_repo({"tests/fixtures/tmux/a.bin": b"Tandem\n", "LICENSE": b"Tandem\n"})
        res = self.run_apply(root, TABLE, "tests/fixtures/tmux/a.bin", "LICENSE")
        self.assertEqual(res.returncode, 0, res.stdout + res.stderr)
        self.assertIn("every named file is a keep file", res.stdout)

    def test_write_is_atomic_rename(self):
        root = make_repo({"a.txt": b"Tandem here\n"})
        tp = os.path.join(root, "t.toml")
        with open(tp, "w") as f:
            f.write("[[row]]\ngroup='A'\nold='Tandem'\nnew='Termurion'\nmode='exact'\nscope=['**']\nreason='x'\n")
        os.chmod(os.path.join(root, "a.txt"), 0o750)
        before = os.stat(os.path.join(root, "a.txt")).st_ino
        res = self.run_apply(root, tp, "--diff", os.path.join(root, "o.diff"), "a.txt")
        self.assertEqual(res.returncode, 0, res.stdout + res.stderr)
        st = os.stat(os.path.join(root, "a.txt"))
        self.assertNotEqual(st.st_ino, before, "file was rewritten in place, not by rename")
        self.assertEqual(st.st_mode & 0o777, 0o750)
        self.assertEqual(slurp(os.path.join(root, "a.txt")), "Termurion here\n")
        self.assertEqual([n for n in os.listdir(root) if n.startswith(".rename-tmp")], [])


if __name__ == "__main__":
    unittest.main()
