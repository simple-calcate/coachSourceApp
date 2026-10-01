"""生成应用图标源图。运行：python3 tools/make_icon.py"""
from PIL import Image, ImageDraw

S = 1024
img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
d = ImageDraw.Draw(img)

d.rounded_rectangle([0, 0, S - 1, S - 1], radius=224, fill=(23, 27, 34, 255))

pad = 190
d.ellipse([pad, pad, S - pad, S - pad], fill=(127, 119, 221, 255))

bx0, by0, bx1, by1 = 340, 400, 684, 560
d.rounded_rectangle([bx0, by0, bx1, by1], radius=44, fill=(255, 255, 255, 255))
d.polygon([(bx0 + 40, by1 - 6), (bx0 + 40, by1 + 76), (bx0 + 150, by1 - 6)], fill=(255, 255, 255, 255))

d.rounded_rectangle([560, 560, 760, 672], radius=36, fill=(206, 203, 246, 255))
d.polygon([(740, 666), (740, 726), (668, 666)], fill=(206, 203, 246, 255))

img.save("icon-source.png")
print("已生成 icon-source.png", img.size)
