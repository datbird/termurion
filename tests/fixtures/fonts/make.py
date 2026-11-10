# Builds a two-member test family: regular from DejaVu Sans Mono, bold from DejaVu Sans Mono Bold
# with every advance 10 percent wider. ASCII only.
import sys
from fontTools import subset
from fontTools.ttLib import TTFont
out = sys.argv[1]
FAMILY = "Termurion Wide Bold Test"
for src, style, weight, widen in [
    ("/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf", "Regular", 400, 1.0),
    ("/usr/share/fonts/truetype/dejavu/DejaVuSansMono-Bold.ttf", "Bold", 700, 1.1),
]:
    f = TTFont(src)
    opts = subset.Options(); opts.name_IDs = ["*"]; opts.layout_features = []; opts.hinting = False
    s = subset.Subsetter(opts); s.populate(unicodes=range(0x20, 0x7f)); s.subset(f)
    hmtx = f["hmtx"]
    for g, (adv, lsb) in list(hmtx.metrics.items()):
        hmtx.metrics[g] = (round(adv * widen), lsb)
    f["hhea"].advanceWidthMax = max(a for a, _ in hmtx.metrics.values())
    f["OS/2"].usWeightClass = weight
    f["OS/2"].xAvgCharWidth = round(f["OS/2"].xAvgCharWidth * widen)
    name = f["name"]
    ps = FAMILY.replace(" ", "") + "-" + style
    for rec in list(name.names):
        if rec.nameID in (16, 17, 21, 22):
            name.removeNames(nameID=rec.nameID)
    for nid, val in [(1, FAMILY), (2, style), (3, ps), (4, FAMILY + " " + style), (6, ps)]:
        name.setName(val, nid, 3, 1, 0x409); name.setName(val, nid, 1, 0, 0)
    f.save(f"{out}/TermurionWideBoldTest-{style}.ttf")

# A ligature test font: DejaVu Sans Mono with "=>" and "===" each one glyph (the arrow), one cell
# wide, so a ligature that covers several characters advances less than its cells.
from fontTools.feaLib.builder import addOpenTypeFeaturesFromString
FAMILY = "Termurion Ligature Test"
f = TTFont("/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf")
opts = subset.Options(); opts.name_IDs = ["*"]; opts.layout_features = []; opts.hinting = False
s = subset.Subsetter(opts); s.populate(unicodes=list(range(0x20, 0x7f)) + [0x2192, 0xe9]); s.subset(f)
if "GSUB" in f:
    del f["GSUB"]
# "é" is made 1.5 cells wide: a glyph whose advance is not the font's "W".
adv, lsb = f["hmtx"]["eacute"]
f["hmtx"]["eacute"] = (round(adv * 1.5), lsb)
addOpenTypeFeaturesFromString(f, "languagesystem DFLT dflt; languagesystem latn dflt;\nfeature liga { sub equal greater by arrowright; sub equal equal equal by arrowright; } liga;")
name = f["name"]
ps = FAMILY.replace(" ", "") + "-Regular"
for nid in (16, 17, 21, 22):
    name.removeNames(nameID=nid)
for nid, val in [(1, FAMILY), (2, "Regular"), (3, ps), (4, FAMILY + " Regular"), (6, ps)]:
    name.setName(val, nid, 3, 1, 0x409); name.setName(val, nid, 1, 0, 0)
f.save(f"{out}/TermurionLigatureTest-Regular.ttf")

# A right-to-left test font: DejaVu Sans with its Hebrew and Arabic letters and its own shaping
# features (Arabic joining forms), renamed.
FAMILY = "Termurion RTL Test"
f = TTFont("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf")
opts = subset.Options(); opts.name_IDs = ["*"]; opts.layout_features = ["*"]; opts.hinting = False
s = subset.Subsetter(opts); s.populate(unicodes=list(range(0x20, 0x7f)) + list(range(0x5d0, 0x5eb)) + list(range(0x621, 0x64b))); s.subset(f)
name = f["name"]
ps = FAMILY.replace(" ", "") + "-Regular"
for nid in (16, 17, 21, 22):
    name.removeNames(nameID=nid)
for nid, val in [(1, FAMILY), (2, "Regular"), (3, ps), (4, FAMILY + " Regular"), (6, ps)]:
    name.setName(val, nid, 3, 1, 0x409); name.setName(val, nid, 1, 0, 0)
f.save(f"{out}/TermurionRtlTest-Regular.ttf")
