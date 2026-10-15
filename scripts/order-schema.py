#!/usr/bin/env python3
"""Reorders the rows of schema/profile.toml and schema/global.toml into the order iTerm2's Settings xibs
draw them (top to bottom, then left to right), using the positions scripts/gen-ui-labels.py wrote into
schema/ui_labels.toml. Rows with no position stay right after the row they followed. A group of keys one
function draws together (the font block, the window size line) takes the position of its lead key.
`src/ui_labels.rs` holds the result to the same rule. Usage: scripts/order-schema.py [--check]"""
import functools, os, re, sys, tomllib

here = os.path.dirname(os.path.abspath(__file__))
schema = os.path.join(here, "..", "schema")
LABELS = tomllib.load(open(os.path.join(schema, "ui_labels.toml"), "rb"))["control"]
POS = {(c["scope"], c["key"]): c for c in LABELS if "pane" in c}
PROFILE_PANES = {
    "ProfilesGeneralPreferencesViewController": "General",
    "ProfilesColorsPreferencesViewController": "Colors",
    "ProfilesTextPreferencesViewController": "Text",
    "ProfilesWindowPreferencesViewController": "Window",
    "ProfilesTerminalPreferencesViewController": "Terminal",
    "ProfilesSessionPreferencesViewController": "Session",
    "ProfilesKeysPreferencesViewController": "Keys",
    "ProfilesAdvancedPreferencesViewController": "Advanced",
}
GLOBAL_PANES = {
    "GeneralPreferencesViewController": "General",
    "AppearancePreferencesViewController": "Appearance",
    "KeysPreferencesViewController": "Keys",
    "PointerPreferencesViewController": "Pointer",
}
# Keys one drawing covers: the key whose position the others take.
LEAD = {
    "Non Ascii Font": "Normal Font", "Use Non-ASCII Font": "Normal Font", "Horizontal Spacing": "Normal Font", "Vertical Spacing": "Normal Font",
    "ASCII Anti Aliased": "Normal Font", "Non-ASCII Anti Aliased": "Normal Font", "ASCII Ligatures": "Normal Font", "Non-ASCII Ligatures": "Normal Font",
    "Rows": "Columns", "Width Percentage": "Columns", "Height Percentage": "Columns", "Title Function": "Title Components",
}
ROW = 6.0


def lead(key):
    return LEAD.get(key, key)


def position(scope, key, tab):
    panes = PROFILE_PANES if scope == "profile" else GLOBAL_PANES
    for k in (lead(key), key):
        c = POS.get((scope, k))
        if c and panes.get(c["pane"]) == tab:
            return (c.get("item_index", 0), c["top"], c["left"])
    return None


def before(a, b):
    """Negative when position a draws above b."""
    if a[0] != b[0]:
        return a[0] - b[0]
    if abs(a[1] - b[1]) > ROW:
        return -1 if a[1] > b[1] else 1
    return -1 if a[2] < b[2] else (1 if a[2] > b[2] else 0)


def blocks(text):
    parts = re.split(r"(?m)^(?=\[\[setting\]\]$)", text)
    return parts[0], parts[1:]


def reorder(scope, groups_of):
    path = os.path.join(schema, f"{scope}.toml")
    text = open(path, encoding="utf-8").read()
    head, bl = blocks(text)
    info = []
    for b in bl:
        key = re.search(r'(?m)^key = "((?:[^"\\]|\\.)*)"', b).group(1)
        tab = (re.search(r'(?m)^tab = "([^"]*)"', b) or [None, ""])[1]
        sec = (re.search(r'(?m)^section = "([^"]*)"', b) or [None, ""])[1]
        info.append((key, tab, sec))
    done = set()
    new = list(bl)
    for i, (key, tab, sec) in enumerate(info):
        group = groups_of(tab, sec)
        if group is None or group in done:
            continue
        done.add(group)
        slots = [j for j, (k, t, s) in enumerate(info) if groups_of(t, s) == group]
        members = [(j, info[j][0]) for j in slots]
        # A row with no position stays with the positioned row before it in the file (a leading run of them
        # stays first); the positioned rows, each with its followers, are sorted.
        units = []
        for n, (j, k) in enumerate(members):
            p = position(scope, k, info[j][1])
            if p is not None or not units:
                units.append([(j, k, p, n)])
            else:
                units[-1].append((j, k, p, n))
        positioned = [u for u in units if u[0][2] is not None]
        loose = [u for u in units if u[0][2] is None]
        positioned.sort(key=functools.cmp_to_key(lambda u, v: before(u[0][2], v[0][2]) or (u[0][3] - v[0][3])))
        ordered = loose + positioned
        flat = [e for u in ordered for e in u]
        for slot, e in zip(slots, flat):
            new[slot] = bl[e[0]]
    out = head + "".join(new)
    changed = out != text
    if "--check" in sys.argv:
        print(f"{scope}: {'out of order' if changed else 'in order'}")
        return changed
    open(path, "w", encoding="utf-8").write(out)
    print(f"{scope}: {'reordered' if changed else 'already in order'}")
    return changed


reorder("profile", lambda tab, sec: tab if tab in PROFILE_PANES.values() else None)
reorder("global", lambda tab, sec: (tab, sec) if tab in GLOBAL_PANES.values() else None)
