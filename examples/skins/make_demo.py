"""重新生成 examples/skins/demo 的图：内置立绘换个色相 + 缩到 300×440（故意和 400×587 不一样，演示自动缩放）。
用法（仓库根目录）：python examples/skins/make_demo.py   需要 Pillow。
"""
import os
from PIL import Image, ImageChops, ImageFilter
here = os.path.dirname(os.path.abspath(__file__))
src = os.path.join(here, "..", "..", "assets", "sprites")
out = os.path.join(here, "demo")
os.makedirs(out, exist_ok=True)
SHIFT = 110  # 0-255 hue units (~155 deg)
SIZE = (300, 440)  # deliberately not 400x587: demonstrates auto-scale

def hue(img):
    img = img.convert("RGBA")
    r, g, b, a = img.split()
    h, s, v = Image.merge("RGB", (r, g, b)).convert("HSV").split()
    h = h.point(lambda x: (x + SHIFT) % 256)
    rgb = Image.merge("HSV", (h, s, v)).convert("RGB")
    return Image.merge("RGBA", (*rgb.split(), a))

def fit(img):
    return img.resize(SIZE, Image.LANCZOS)

for st in ["idle", "happy", "sleep", "think"]:
    im = Image.open(os.path.join(src, st + ".png"))
    print(st, im.size, im.mode)
    fit(hue(im)).save(os.path.join(out, st + ".png"), optimize=True)

idle = Image.open(os.path.join(src, "idle.png")).convert("RGBA")
blink = Image.open(os.path.join(src, "blink.png")).convert("RGBA")
assert idle.size == blink.size
diff = ImageChops.difference(idle, blink).convert("L").point(lambda x: 255 if x > 24 else 0)
bbox = diff.getbbox(); print("blink diff bbox", bbox)
# 叠加层：只保留和待机不同的眼睛，其余透明（和立绘同尺寸，直接盖上去就对齐）
# 只取眼睛那一块（原图坐标），别的地方两张图有细微差别，不要
EYES = (90, 160, 250, 250)
eyes = Image.new("L", diff.size, 0)
eyes.paste(diff.crop(EYES).filter(ImageFilter.MaxFilter(5)), EYES[:2])
mask = eyes
ov = Image.new("RGBA", blink.size, (0, 0, 0, 0))
ov.paste(blink, (0, 0), mask)
fit(hue(ov)).save(os.path.join(out, "blink.png"), optimize=True)
for f in sorted(os.listdir(out)):
    print(f, os.path.getsize(os.path.join(out, f)))
