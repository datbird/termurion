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
