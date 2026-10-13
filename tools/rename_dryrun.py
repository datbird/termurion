#!/usr/bin/env python3
"""Dry run for the m14r rename: list every case-insensitive "tandem" hit with its table row.

Writes no file in the repo (use shell redirection for the report). Read-only.

  python3 tools/rename_dryrun.py [--root DIR] [--table FILE] [--list] [--unmapped-only]

Default output is a summary: totals, per-group counts, per-row counts, per-path-rule counts, and
every UNMAPPED hit outside keep-history paths. --list prints every hit (file:line:col, row).
Exit status is 1 when any hit outside keep-history and keep paths is UNMAPPED.
"""
import argparse
import os
import sys
from collections import Counter, defaultdict
from dataclasses import dataclass, field

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import rename_core as core  # noqa: E402

DEFAULT_TABLE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "rename_table.toml")


@dataclass
class Report:
    total_lines: int = 0
    total_hits: int = 0
    files_with_hits: int = 0
    unmapped_outside_history: int = 0
    by_group: Counter = field(default_factory=Counter)  # group -> hits (mapped rows)
    by_row: Counter = field(default_factory=Counter)  # (group, mode, old, new) -> hits
    by_pathrule: Counter = field(default_factory=Counter)  # (glob, mode) -> hits
    keep_hits: int = 0
    history_hits: int = 0
    unmapped: list = field(default_factory=list)  # (file, line, col, snippet)
    listing: list = field(default_factory=list)  # (file, line, col, label)
    path_hits: list = field(default_factory=list)  # file names that contain the name


def scan(root, table_path):
    table = core.load_table(table_path)
    rep = Report()
    for rel in core.tracked_files(root):
        if core.HIT_RE.search(rel):
            rep.path_hits.append(rel)
        raw, text = core.read_text(root, rel)
        if not core.HIT_RE.search(text):
            continue
        prule = core.path_rule(table, rel)
        rows = None if prule else core.rows_for(table, rel)
        file_hits = 0
        for ln, line in enumerate(text.split("\n"), 1):
            if not core.HIT_RE.search(line):
                continue
            rep.total_lines += 1
            if prule:
                for m in core.HIT_RE.finditer(line):
                    file_hits += 1
                    rep.total_hits += 1
                    rep.by_pathrule[(prule.glob, prule.mode)] += 1
                    if prule.mode == "keep":
                        rep.keep_hits += 1
                    else:
                        rep.history_hits += 1
                    rep.listing.append((rel, ln, m.start() + 1, "PATH-" + prule.mode.upper() + " " + prule.glob))
                continue
            for col, row in core.classify_hits(table, rel, line, rows):
                file_hits += 1
                rep.total_hits += 1
                if row is None:
                    rep.unmapped_outside_history += 1
                    rep.unmapped.append((rel, ln, col + 1, line.strip()[:160]))
                    rep.listing.append((rel, ln, col + 1, "UNMAPPED"))
                    continue
                if row.mode == "keep":
                    rep.keep_hits += 1
                    rep.by_group["K"] += 1
                else:
                    rep.by_group[row.group] += 1
                rep.by_row[(row.group, row.mode, row.old, row.new)] += 1
                rep.listing.append((rel, ln, col + 1, "%s %s -> %s" % (row.group, row.old, row.new)))
        if file_hits:
            rep.files_with_hits += 1
    return rep


def render(rep, table_path, listing=False):
    out = []
    w = out.append
    w("m14r rename dry run (read only; no file written by the tool)")
    w("table: %s" % os.path.basename(table_path))
    w("")
    w("total hits (case-insensitive 'tandem'): %d" % rep.total_hits)
    w("matching lines: %d" % rep.total_lines)
    w("files with hits: %d" % rep.files_with_hits)
    w("UNMAPPED outside keep and keep-history paths: %d" % rep.unmapped_outside_history)
    w("")
    w("per-group hits mapped by a rename row (A user strings, B identifiers, C data paths,")
    w("D code identifiers, E docs and scripts, F assets, K keep rows):")
    for g in "ABCDEFKT":
        w("  %s: %d" % (g, rep.by_group.get(g, 0)))
    w("  whole-file keep paths: %d" % sum(n for (gl, m), n in rep.by_pathrule.items() if m == "keep"))
    w("  keep-history paths:    %d" % sum(n for (gl, m), n in rep.by_pathrule.items() if m == "keep-history"))
    w("")
    w("per path rule:")
    for (glob, mode), n in sorted(rep.by_pathrule.items()):
        w("  %-14s %6d  %s" % (mode, n, glob))
    w("")
    w("per row (group, mode, old -> new, hits), most used first:")
    for (g, mode, old, new), n in sorted(rep.by_row.items(), key=lambda kv: (-kv[1], kv[0])):
        w("  %s %-5s %6d  %s -> %s" % (g, mode, n, old, new if mode != "keep" else "(keep)"))
    w("")
    w("file names that contain the name (renamed by git mv in their tasks): %d" % len(rep.path_hits))
    for p in rep.path_hits:
        w("  " + p)
    w("")
    w("UNMAPPED hits outside keep-history paths: %d" % len(rep.unmapped))
    for rel, ln, col, snip in rep.unmapped:
        w("  UNMAPPED %s:%d:%d: %s" % (rel, ln, col, snip))
    if listing:
        w("")
        w("every hit:")
        for rel, ln, col, label in rep.listing:
            w("  %s:%d:%d  %s" % (rel, ln, col, label))
    return "\n".join(out) + "\n"


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--root", default=os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    ap.add_argument("--table", default=DEFAULT_TABLE)
    ap.add_argument("--list", action="store_true", help="print every hit")
    args = ap.parse_args(argv)
    rep = scan(args.root, args.table)
    sys.stdout.write(render(rep, args.table, args.list))
    return 1 if rep.unmapped_outside_history else 0


if __name__ == "__main__":
    sys.exit(main())
