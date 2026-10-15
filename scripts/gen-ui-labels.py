#!/usr/bin/env python3
"""Builds schema/ui_labels.toml from iTerm2's Settings xibs (read-only reference clone).

For every control a Settings view controller binds with defineControl (a plist key), this finds
the control's label in PreferencePanel.xib: the relatedView outlet, the <name>Label outlet, the
control's own title (checkbox, radio), or else the static text to its left or above it in the
same box. It also records a popup's menu item titles. The guard test in src/ui_labels.rs holds
the schema rows to this table. Usage: scripts/gen-ui-labels.py [path/to/iterm2-ref]
"""
import os
import re
import sys
import xml.etree.ElementTree as ET

here = os.path.dirname(os.path.abspath(__file__))
ref = sys.argv[1] if len(sys.argv) > 1 else os.environ.get("ITERM2_REF") or os.path.join(here, "..", "..", "iterm2-ref")
settings = f"{ref}/sources/Settings"
xib_path = f"{settings}/Base.lproj/PreferencePanel.xib"
root = ET.parse(xib_path).getroot()

# ---- plist key constants -------------------------------------------------------------------
consts = {}
for fn in ("Profiles/ITAddressBookMgr.h", "iTermPreferences.m", "iTermPreferences.h", "iTermProfilePreferences.m"):
    p = f"{settings}/{fn}"
    if not os.path.exists(p):
        continue
    t = open(p, encoding="utf-8", errors="replace").read()
    for m in re.finditer(r'#define\s+(KEY_\w+)\s+@"([^"]*)"', t):
        consts[m.group(1)] = m.group(2)
    for m in re.finditer(r'NSString\s*\*\s*const\s+(\w+)\s*=\s*@"([^"]*)"', t):
        consts[m.group(1)] = m.group(2)

# ---- xib model -------------------------------------------------------------------------------
byid = {}
parent = {}
for el in root.iter():
    for ch in el:
        parent[ch] = el
    if el.get("id"):
        byid[el.get("id")] = el


def frame(el):
    r = el.find("rect[@key='frame']")
    if r is None:
        return None
    return float(r.get("x", 0)), float(r.get("y", 0)), float(r.get("width", 0)), float(r.get("height", 0))


def abs_frame(el):
    """Frame in the coordinates of the top view, y still AppKit-style (up). Boxes add 0."""
    f = frame(el)
    if f is None:
        return None
    x, y, w, h = f
    p = parent.get(el)
    while p is not None:
        if p.tag in ("view", "customView", "box", "tabViewItem", "scrollView", "clipView", "stackView"):
            pf = frame(p)
            if pf is not None and p.tag != "box":
                x += pf[0]
                y += pf[1]
        p = parent.get(p)
    return x, y, w, h


def pick(d, *names):
    for n in names:
        if d.get(n) is not None:
            return d[n]
    return None


def text_of(el):
    if el is None:
        return None
    if el.tag == "button":
        c = el.find("buttonCell")
        return (c.get("title") if c is not None else None) or el.get("title")
    if el.tag == "textField":
        c = el.find("textFieldCell")
        return c.get("title") if c is not None else None
    if el.tag == "box":
        return el.get("title")
    return None


def popup_items(el):
    out = []
    if el is None or el.tag != "popUpButton":
        return out
    for mi in el.iter("menuItem"):
        if mi.get("isSeparatorItem") == "YES":
            continue
        out.append((mi.get("tag"), mi.get("title") or ""))
    return out


def matrix_items(el):
    out = []
    if el is None or el.tag != "matrix":
        return out
    for c in el.iter("buttonCell"):
        out.append((c.get("tag"), c.get("title") or ""))
    return out


def container(el):
    """The nearest ancestor that holds the control's siblings (the view with <subviews>)."""
    p = parent.get(el)
    while p is not None and p.tag == "subviews":
        p = parent.get(p)
    return p


def nearby_static_text(el):
    """Static text left of el on the same row, else the nearest one directly above it."""
    cont = container(el)
    if cont is None:
        return None
    sub = cont.find("subviews")
    if sub is None:
        return None
    f = frame(el)
    if f is None:
        return None
    ex, ey, ew, eh = f
    cy = ey + eh / 2
    best = None
    for s in sub:
        if s is el or s.tag != "textField":
            continue
        c = s.find("textFieldCell")
        if c is None or c.get("title") in (None, "", "Label", "Text Cell"):
            continue
        if s.get("editable") == "YES" or c.get("editable") == "YES":
            continue
        sf = frame(s)
        if sf is None:
            continue
        sx, sy, sw, sh = sf
        scy = sy + sh / 2
        colon = c.get("title").rstrip().endswith(":")
        # Text that ends in a colon may sit a little off the row; other text must be right on it,
        # touching the control, and start with a capital (not a fragment such as "columns by").
        tight = abs(scy - cy) <= 6 and ex - (sx + sw) <= 20 and c.get("title")[:1].isupper() and len(c.get("title")) > 12
        if (colon and abs(scy - cy) <= 16 or tight) and sx + sw <= ex + 6:
            d = ex - (sx + sw)
            if best is None or d < best[0]:
                best = (d, c.get("title"))
    return best[1] if best else None


# ---- outlets per view controller class ------------------------------------------------------
outlets = {}
for co in root.iter("customObject"):
    cls = co.get("customClass")
    if not cls:
        continue
    d = outlets.setdefault(cls, {})
    for o in co.iterfind("connections/outlet"):
        d[o.get("property")] = byid.get(o.get("destination"))
for vc in root.iter("viewController"):
    cls = vc.get("customClass")
    if not cls:
        continue
    d = outlets.setdefault(cls, {})
    for o in vc.iterfind("connections/outlet"):
        d[o.get("property")] = byid.get(o.get("destination"))

# ---- defineControl calls ----------------------------------------------------------------------
call = re.compile(r"define(?:Unsearchable)?Control:\s*(?:self\.|_)?(\w+)\s+key:\s*(KEY_\w+|kPreferenceKey\w+|@\"[^\"]+\")(.*?)type:\s*(\w+)", re.S)
rows = []
seen = set()
for fn in sorted(os.listdir(settings)):
    if not fn.endswith(".m"):
        continue
    cls = fn[:-2]
    if cls not in outlets:
        continue
    text = open(f"{settings}/{fn}", encoding="utf-8", errors="replace").read()
    for m in call.finditer(text):
        name, keytok, mid, typ = m.groups()
        key = keytok[2:-1] if keytok.startswith("@") else consts.get(keytok)
        if not key:
            continue
        line = text.count("\n", 0, m.start()) + 1
        d = outlets[cls]
        el = pick(d, f"_{name}", name)
        rel = re.search(r"relatedView:\s*(?:self\.|_)?(\w+)", mid)
        label = None
        how = ""
        if rel and pick(d, f"_{rel.group(1)}", rel.group(1)) is not None:
            label = text_of(pick(d, f"_{rel.group(1)}", rel.group(1)))
            how = "related"
        if label is None:
            lo = pick(d, f"_{name}Label", f"{name}Label")
            label = text_of(lo)
            how = "label-outlet"
        scope = "profile" if cls.startswith("Profiles") else "global"
        if label is None and el is not None and el.tag == "button":
            label = text_of(el)
            how = "title"
        if label is None and el is not None:
            label = nearby_static_text(el)
            how = "adjacent"
        items = popup_items(el) or matrix_items(el)
        ident = (scope, key)
        if ident in seen:
            continue
        seen.add(ident)
        rows.append((scope, key, label, how, items, f"{fn}:{line}", typ, name))


smooth = re.compile(r"defineSmoothBlink\w+:\s*_(\w+)\s+label:\s*_(\w+)\s+key:\s*(KEY_\w+)")
for fn in ("ProfilesTextPreferencesViewController.m",):
    text = open(f"{settings}/{fn}", encoding="utf-8", errors="replace").read()
    d = outlets["ProfilesTextPreferencesViewController"]
    for m in smooth.finditer(text):
        name, lab, keytok = m.groups()
        key = consts.get(keytok)
        if (("profile", key)) in seen or not key:
            continue
        seen.add(("profile", key))
        line = text.count("\n", 0, m.start()) + 1
        rows.append(("profile", key, text_of(pick(d, f"_{lab}")), "label-outlet", popup_items(pick(d, f"_{name}")), f"{fn}:{line}", "smooth", name))

# Color wells: two dictionary literals, controls and related labels.
fn = "ProfilesColorsPreferencesViewController.m"
text = open(f"{settings}/{fn}", encoding="utf-8", errors="replace").read()
d = outlets["ProfilesColorsPreferencesViewController"]


def dict_pairs(name):
    m = re.search(name + r"\s*\{\s*return @\{(.*?)\};", text, re.S)
    return re.findall(r"(KEY_\w+):\s*_(\w+)", m.group(1)) if m else []


related = dict(dict_pairs(r"colorWellRelatedViews"))
for keytok, name in dict_pairs(r"colorWellDictionary"):
    key = consts.get(keytok)
    if not key or ("profile", key) in seen:
        continue
    seen.add(("profile", key))
    lab = related.get(keytok)
    label = text_of(pick(d, f"_{lab}")) if lab else None
    el = pick(d, f"_{name}")
    how = "related"
    if label is None and el is not None:
        label = nearby_static_text(el)
        how = "adjacent"
    rows.append(("profile", key, label, how, [], f"{fn}", "color", name))


# Controls iTerm2 binds some other way (a font picker, a radio matrix in a box, a sheet with its own
# xib) whose label is a static text in an xib. Each text is looked up in its xib below, so a typo
# cannot get in. The value is the text (in PreferencePanel.xib) or (xib file, text).
SHEETS = {
    "badge": "iTermBadgeConfigurationWindowController.xib",
    "gpu": "iTermAdvancedGPUSettingsWindowController.xib",
    "triggers": "iTermTriggersPanel.xib",
}
EXTRA = {
    ("profile", "Normal Font"): "Font:",
    ("profile", "Non Ascii Font"): "Non-ASCII Font:",
    ("profile", "Special Font Config"): "Manage Special Exceptions\u2026",
    ("profile", "Blur Radius"): "Blur:",
    ("profile", "Blend"): "Blending:",
    ("profile", "Badge Top Margin"): ("badge", "Top Margin:"),
    ("profile", "Badge Right Margin"): ("badge", "Right Margin:"),
    ("profile", "Badge Max Width"): ("badge", "Maximum Width:"),
    ("profile", "Badge Max Height"): ("badge", "Maximum Height:"),
    ("profile", "Triggers Use Interpolated Strings"): ("triggers", "Use interpolated strings for parameters"),
    ("profile", "Smart Selection Actions Use Interpolated Strings"): "Use interpolated strings for parameters (affects all actions)",
    ("profile", "Dev Null Mode"): "Use /dev/null mode (no history, cookies, or other data saved)",
    ("profile", "Instant Replay"): "Enable Instant Replay (costs memory and CPU)",
    ("profile", "Browser Zoom"): "Page Zoom:",
    ("profile", "Session Close Undo Timeout"): "\u201cUndo\u201d can revive a closed session for up to this many seconds:",
    ("global", "API Authentication Method"): "Python API:",
    ("global", "WindowPlacement"): "New window placement:",
    ("global", "TmuxPauseModeAgeLimit"): "Pause a pane if catching up would take longer than (seconds):",
    ("global", "OpenTmuxWindowsIn"): "When attaching, open unrecognized windows in",
    ("global", "disableMetalWhenUnplugged"): ("gpu", "Disable GPU renderer when disconnected from power"),
    ("global", "disableMetalInLowPowerMode"): ("gpu", "Disable GPU renderer in low power mode"),
    ("global", "preferIntegratedGPU"): ("gpu", "Prefer integrated to discrete GPU"),
}


def xib_texts(path):
    r = ET.parse(path).getroot()
    return {e.get("title") for e in r.iter() if e.tag in ("textFieldCell", "buttonCell") and e.get("title")}


texts = {None: xib_texts(xib_path)}
for name, fn in SHEETS.items():
    texts[name] = xib_texts(f"{settings}/Base.lproj/{fn}")


def extra_label(sc, k):
    v = EXTRA[(sc, k)]
    sheet, text = v if isinstance(v, tuple) else (None, v)
    assert text in texts[sheet], text
    return text, (SHEETS[sheet] if sheet else "PreferencePanel.xib")


for i, r in enumerate(rows):
    if (r[0], r[1]) in EXTRA:
        text, src = extra_label(r[0], r[1])
        rows[i] = (r[0], r[1], text, "extra", r[4], src, r[6], r[7])
for (sc, k) in EXTRA:
    if not any(r[0] == sc and r[1] == k for r in rows):
        text, src = extra_label(sc, k)
        rows.append((sc, k, text, "extra", [], src, "extra", ""))


def q(s):
    s = s.replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n").replace("\r", "\\r").replace("\t", "\\t")
    return '"' + s + '"'


out = [
    "# Generated by scripts/gen-ui-labels.py from iTerm2's Settings xibs and view controllers. Do not edit by hand.",
    "# Each row: the plist key a control binds, the label iTerm2 draws for it (empty when the xib has none),",
    "# how the label was found, and the popup or radio titles. `source` is the defineControl call.",
    "",
]
for scope, key, label, how, items, src, typ, name in rows:
    out.append("[[control]]")
    out.append(f"scope = {q(scope)}")
    out.append(f"key = {q(key)}")
    out.append(f"label = {q(label or '')}")
    out.append(f"how = {q(how if label else 'none')}")
    out.append(f"outlet = {q(name)}")
    out.append(f"type = {q(typ)}")
    out.append(f"source = {q(src)}")
    if items:
        out.append("choices = [" + ", ".join(f"[{q(t or '')}, {q(l)}]" for t, l in items) + "]")
    out.append("")
open(os.path.join(here, "..", "schema", "ui_labels.toml"), "w", encoding="utf-8").write("\n".join(out))
print(len(rows), "controls,", sum(1 for r in rows if r[2]), "with a label")
