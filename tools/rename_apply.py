#!/usr/bin/env python3
"""Apply the m14r table rows to the files a task names. Never a blanket replace.

  python3 tools/rename_apply.py [--root DIR] [--table FILE] [--groups B,D] [--check]
                                [--diff OUT] [--backup DIR] FILE [FILE ...]

* Only table rows are applied (exact literals; keep rows are never changed).
* If any named file has a hit with no table row (UNMAPPED), the tool exits non-zero and changes
  nothing at all, not even the other files.
* Files under a keep or keep-history path rule are skipped and reported, never written.
* --groups limits the rows applied to the listed groups; hits owned by other groups are left.
* --check reports what would change and writes nothing.
* After writing, a unified diff against the backup tree (default .rename-backup/, falling back
  to the pre-apply content for a file the backup lacks) goes to --diff
  (default .superpowers/sdd/tweaks/termurion-apply.diff).
"""
import argparse
import difflib
import os
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import rename_core as core  # noqa: E402

DEFAULT_TABLE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "rename_table.toml")
DEFAULT_DIFF = os.path.join(".superpowers", "sdd", "tweaks", "termurion-apply.diff")


def plan_file(table, rel, raw, text, groups):
    """(new_text, changes, unmapped) for one file. changes = number of replacements."""
    rows = core.rows_for(table, rel)
    sel = core.resolve(table, rel, text, rows)
    unmapped = []
    for ln, line in enumerate(text.split("\n"), 1):
        if not core.HIT_RE.search(line):
            continue
        for col, row in core.classify_hits(table, rel, line, rows):
            if row is None:
                unmapped.append((rel, ln, col + 1, line.strip()[:160]))
    out = []
    pos = 0
    changes = 0
    for s in sel:
        if core.is_keep(s.row) or (groups and s.row.group not in groups):
            continue
        out.append(text[pos : s.start])
        out.append(s.row.new)
        pos = s.end
        changes += 1
    out.append(text[pos:])
    return "".join(out), changes, unmapped


def write_atomic(full, data):
    """Write through a temp file in the same directory, then rename over the target (mode kept)."""
    d = os.path.dirname(full)
    fd, tmp = tempfile.mkstemp(prefix=".rename-tmp-", dir=d)
    try:
        with os.fdopen(fd, "wb") as f:
            f.write(data)
            f.flush()
            os.fsync(f.fileno())
        os.chmod(tmp, os.stat(full).st_mode & 0o7777)
        os.replace(tmp, full)
    except BaseException:
        if os.path.exists(tmp):
            os.unlink(tmp)
        raise


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("files", nargs="+")
    ap.add_argument("--root", default=os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    ap.add_argument("--table", default=DEFAULT_TABLE)
    ap.add_argument("--groups", default="")
    ap.add_argument("--check", action="store_true")
    ap.add_argument("--diff", default=None)
    ap.add_argument("--backup", default=None)
    args = ap.parse_args(argv)

    root = os.path.abspath(args.root)
    table = core.load_table(args.table)
    groups = set(g for g in args.groups.split(",") if g)
    backup = args.backup or os.path.join(root, ".rename-backup")
    diff_out = args.diff or os.path.join(root, DEFAULT_DIFF)

    plans = []
    all_unmapped = []
    for rel in args.files:
        rel = os.path.relpath(os.path.join(root, rel), root).replace(os.sep, "/")
        full = os.path.join(root, rel)
        if not os.path.isfile(full):
            print("error: not a file: %s" % rel, file=sys.stderr)
            return 2
        prule = core.path_rule(table, rel)
        if prule:
            print("skip %s (%s: %s)" % (rel, prule.mode, prule.glob))
            continue
        raw, text = core.read_text(root, rel)
        new_text, changes, unmapped = plan_file(table, rel, raw, text, groups)
        all_unmapped.extend(unmapped)
        plans.append((rel, full, text, new_text, changes))

    if all_unmapped:
        for rel, ln, col, snip in all_unmapped:
            print("UNMAPPED %s:%d:%d: %s" % (rel, ln, col, snip))
        print("refused: %d UNMAPPED hit(s); nothing was changed" % len(all_unmapped), file=sys.stderr)
        return 2

    if plans == [] and args.files:
        print("notice: every named file is a keep file (a keep or keep-history path rule); nothing to rename, nothing written")
        return 0
    diffs = []
    total = 0
    for rel, full, text, new_text, changes in plans:
        total += changes
        print("%s %s: %d replacement(s)" % ("would change" if args.check else "change", rel, changes))
        if new_text == text:
            continue
        old_path = os.path.join(backup, rel)
        if os.path.isfile(old_path):
            with open(old_path, "rb") as f:
                before = f.read().decode("latin-1")
        else:
            before = text
        diffs.extend(
            difflib.unified_diff(
                before.splitlines(keepends=True),
                new_text.splitlines(keepends=True),
                fromfile="a/" + rel,
                tofile="b/" + rel,
            )
        )
    if args.check:
        print("check only: %d replacement(s) in %d file(s); nothing written" % (total, len(plans)))
        return 0
    for rel, full, text, new_text, changes in plans:
        if new_text != text:
            write_atomic(full, new_text.encode("latin-1"))
    os.makedirs(os.path.dirname(diff_out), exist_ok=True)
    with open(diff_out, "wb") as f:
        f.write("".join(diffs).encode("latin-1"))
    print("applied %d replacement(s) in %d file(s); diff: %s" % (total, len(plans), diff_out))
    return 0


if __name__ == "__main__":
    sys.exit(main())
