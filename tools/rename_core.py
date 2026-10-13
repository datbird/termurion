"""Shared code for the m14r rename tools: table loading, glob matching, hit resolution.

Rules (see tools/rename_table.toml):
  * Rows are exact literals. There is no regex anywhere.
  * A row applies to a file when one of its scope globs matches the file path.
  * For one line of text, every occurrence of every applicable row is a candidate. The longest
    candidate wins; on equal length a keep row wins; on a further tie the row listed first wins.
    Winners never overlap.
  * A "hit" is a case-insensitive occurrence of the old name. A hit is MAPPED when a winning
    candidate covers it, UNMAPPED otherwise.
  * Path globs mark whole files: keep (never touched) or keep-history (historical record).
"""
import fnmatch
import os
import re
import subprocess
import tomllib
from dataclasses import dataclass, field

NAME = "tandem"
HIT_RE = re.compile(NAME, re.IGNORECASE)
WORD_CHARS = "A-Za-z0-9_"


@dataclass
class Row:
    group: str
    old: str
    new: str
    mode: str  # exact | word | keep
    scope: list
    reason: str


@dataclass
class PathRule:
    glob: str
    mode: str  # keep | keep-history
    reason: str


@dataclass
class Asset:
    path: str
    old: str
    task: str
    reason: str


@dataclass
class Table:
    rows: list = field(default_factory=list)
    paths: list = field(default_factory=list)
    assets: list = field(default_factory=list)


@dataclass
class Selected:
    start: int
    end: int
    row: Row


def load_table(path):
    with open(path, "rb") as f:
        data = tomllib.load(f)
    t = Table()
    for r in data.get("row", []):
        t.rows.append(
            Row(
                group=r["group"],
                old=r["old"],
                new=r.get("new", r["old"] if r["mode"] == "keep" else ""),
                mode=r["mode"],
                scope=list(r.get("scope", ["**"])),
                reason=r.get("reason", ""),
            )
        )
    for p in data.get("path", []):
        t.paths.append(PathRule(p["glob"], p["mode"], p.get("reason", "")))
    for a in data.get("asset", []):
        t.assets.append(Asset(a["path"], a.get("old", ""), a.get("task", ""), a.get("reason", "")))
    return t


def _glob_regex(glob):
    out = []
    i = 0
    while i < len(glob):
        c = glob[i]
        if glob.startswith("**/", i):
            out.append("(?:.*/)?")
            i += 3
            continue
        if glob.startswith("**", i):
            out.append(".*")
            i += 2
            continue
        if c == "*":
            out.append("[^/]*")
        elif c == "?":
            out.append("[^/]")
        else:
            out.append(re.escape(c))
        i += 1
    return re.compile("^" + "".join(out) + "$")


_GLOBS = {}


def glob_match(glob, path):
    rx = _GLOBS.get(glob)
    if rx is None:
        rx = _GLOBS[glob] = _glob_regex(glob)
    return rx.match(path) is not None


def path_rule(table, path):
    for p in table.paths:
        if glob_match(p.glob, path):
            return p
    return None


def rows_for(table, path):
    return [r for r in table.rows if any(glob_match(g, path) for g in r.scope)]


def _occurrences(row, text):
    start = 0
    old = row.old
    while True:
        i = text.find(old, start)
        if i < 0:
            return
        j = i + len(old)
        if row.mode == "word":
            before = text[i - 1] if i > 0 else ""
            after = text[j] if j < len(text) else ""
            if (before and re.match("[" + WORD_CHARS + "]", before)) or (
                after and re.match("[" + WORD_CHARS + "]", after)
            ):
                start = i + 1
                continue
        yield i, j
        start = i + 1


def resolve(table, path, text, rows=None):
    """Winning, non-overlapping row occurrences in text, in text order."""
    rows = rows_for(table, path) if rows is None else rows
    cands = []
    for order, row in enumerate(rows):
        for i, j in _occurrences(row, text):
            cands.append((-(j - i), 0 if row.mode == "keep" else 1, order, i, j, row))
    cands.sort(key=lambda c: c[:5])
    taken = []
    chosen = []
    for _, _, _, i, j, row in cands:
        if any(i < b and a < j for a, b in taken):
            continue
        taken.append((i, j))
        chosen.append(Selected(i, j, row))
    chosen.sort(key=lambda s: s.start)
    return chosen


def classify_hits(table, path, text, rows=None):
    """For each case-insensitive hit in text: (start, row-or-None, selection)."""
    sel = resolve(table, path, text, rows)
    out = []
    for m in HIT_RE.finditer(text):
        owner = None
        for s in sel:
            if s.start <= m.start() and m.end() <= s.end:
                owner = s
                break
        out.append((m.start(), owner.row if owner else None))
    return out


def tracked_files(root):
    res = subprocess.run(
        ["git", "-C", root, "ls-files", "-z", "-c", "-o", "--exclude-standard"],
        check=True,
        capture_output=True,
    )
    names = [n for n in res.stdout.decode("utf-8", "surrogateescape").split("\0") if n]
    return sorted(set(n for n in names if os.path.isfile(os.path.join(root, n))))


def read_text(root, rel):
    """Lossless text view of a file (latin-1 keeps every byte), plus the raw bytes."""
    with open(os.path.join(root, rel), "rb") as f:
        raw = f.read()
    return raw, raw.decode("latin-1")
