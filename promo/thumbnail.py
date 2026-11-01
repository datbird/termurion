"""The promo's YouTube thumbnail (1280x720): six effects' Enter looks on Claude Code's input box,
cropped from the rendered shots, beside the title.

  uv run --with pillow python promo/thumbnail.py   (reads the shots in %TEMP%\\termurion-promo2)
"""
from PIL import Image, ImageDraw, ImageFont, ImageFilter
import os
d = os.path.join(os.environ['TEMP'], 'termurion-promo2')
W, H, LW = 1280, 720, 760
bands = [('drop', 214, 1150), ('tabs', 492, 0), ('grow', 394, 0), ('quick_aurora', 14, 0), ('quick_fireworks', 20, 660), ('roll_embers', 14, 0)]
bh = H // len(bands)
img = Image.new('RGB', (W, H), (14, 15, 19))
for k, (s, f, x) in enumerate(bands):
    src = Image.open(os.path.join(d, s, 'f%05d.png' % f)).convert('RGB')
    ch = 164
    cw = round(ch * LW / bh)
    crop = src.crop((x, 1162, x + cw, 1162 + ch))
    img.paste(crop.resize((LW, bh), Image.LANCZOS), (0, k * bh))
dr = ImageDraw.Draw(img, 'RGBA')
for k in range(1, len(bands)):
    dr.line((0, k * bh, LW, k * bh), fill=(0, 0, 0, 255), width=3)
# the panel fades in over the bands' right edge
for i in range(90):
    dr.line((LW - 90 + i, 0, LW - 90 + i, H), fill=(14, 15, 19, int(255 * (i / 90) ** 1.5)))
dr.rectangle((LW, 0, W, H), fill=(14, 15, 19, 255))
def bah(size, weight):
    f = ImageFont.truetype(r'C:\Windows\Fonts\bahnschrift.ttf', size)
    f.set_variation_by_axes([weight, 100])
    return f
cx = LW + (W - LW) // 2 - 10
layer = Image.new('RGBA', (W, H), (0, 0, 0, 0))
ld = ImageDraw.Draw(layer)
ld.text((cx, 250), '13', font=bah(250, 700), fill=(255, 255, 255, 255), anchor='ms')
glow = layer.filter(ImageFilter.GaussianBlur(18))
img.paste(Image.new('RGB', (W, H), (120, 180, 255)), (0, 0), glow.getchannel('A').point(lambda v: int(v * 0.6)))
img.paste(layer, (0, 0), layer)
dr = ImageDraw.Draw(img)
dr.text((cx, 340), 'CURSOR', font=bah(78, 700), fill=(255, 255, 255), anchor='ms')
dr.text((cx, 420), 'EFFECTS', font=bah(78, 700), fill=(255, 255, 255), anchor='ms')
dr.text((cx, 510), 'for Claude Code', font=bah(50, 600), fill=(232, 138, 98), anchor='ms')
dr.text((cx, 640), 'TERMURION', font=bah(40, 700), fill=(159, 208, 254), anchor='ms')
dr.text((cx, 680), 'iTerm2 for Windows', font=bah(28, 400), fill=(150, 156, 170), anchor='ms')
img.save(os.path.join(d, 'thumbnail-v2.png'))
