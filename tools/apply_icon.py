#!/usr/bin/env python3
"""把导出的安卓自适应图标分层图应用进 Tauri 安卓工程。

用法:
    python3 tools/apply_icon.py <图标导出目录>

目录里应含 drawable-{mdpi,hdpi,xhdpi,xxhdpi,xxxhdpi}/ic_launcher_{background,foreground}.png
（WorkBuddy 应用图标导出工具的标准产物）。

做四件事:
1. 每个 dpi 合成完整方图替换 mipmap-{dpi}/ic_launcher.png（老设备兜底）
2. 圆形遮罩版替换 ic_launcher_round.png
3. 分层图拷为 mipmap-{dpi}/ic_launcher_foreground.png / ic_launcher_background.png
4. 写 mipmap-anydpi-v26/ic_launcher.xml 与 ic_launcher_round.xml
   —— Android 8+ 启动器走自适应图标，圆角/圆形由启动器统一裁剪
"""
import os
import sys

from PIL import Image, ImageDraw

EXP = sys.argv[1].rstrip("/")
HERE = os.path.dirname(os.path.abspath(__file__))
RES = os.path.join(HERE, "..", "src-tauri", "gen", "android", "app", "src", "main", "res")

# (dpi 目录名, 画布像素)。自适应图标画布是 108dp，1dp=1px@mdpi
DPIS = [("mdpi", 108), ("hdpi", 162), ("xhdpi", 216), ("xxhdpi", 324), ("xxxhdpi", 432)]

ADAPTIVE_XML = """<?xml version="1.0" encoding="utf-8"?>
<adaptive-icon xmlns:android="http://schemas.android.com/apk/res/android">
    <background android:drawable="@mipmap/ic_launcher_background"/>
    <foreground android:drawable="@mipmap/ic_launcher_foreground"/>
</adaptive-icon>
"""


def main() -> None:
    anydpi = os.path.join(RES, "mipmap-anydpi-v26")
    os.makedirs(anydpi, exist_ok=True)
    for name in ("ic_launcher.xml", "ic_launcher_round.xml"):
        with open(os.path.join(anydpi, name), "w", encoding="utf-8") as f:
            f.write(ADAPTIVE_XML)
    print("✓ mipmap-anydpi-v26/ic_launcher.xml, ic_launcher_round.xml")

    for dpi, size in DPIS:
        m = os.path.join(RES, f"mipmap-{dpi}")
        os.makedirs(m, exist_ok=True)
        bg = Image.open(f"{EXP}/drawable-{dpi}/ic_launcher_background.png").convert("RGBA")
        fg = Image.open(f"{EXP}/drawable-{dpi}/ic_launcher_foreground.png").convert("RGBA")
        if bg.size != (size, size) or fg.size != (size, size):
            raise SystemExit(f"尺寸不符: {dpi} bg={bg.size} fg={fg.size} 期望 {(size, size)}")
        full = Image.alpha_composite(bg, fg)

        full.save(f"{m}/ic_launcher.png")

        mask = Image.new("L", (size, size), 0)
        ImageDraw.Draw(mask).ellipse((0, 0, size - 1, size - 1), fill=255)
        round_img = full.copy()
        round_img.putalpha(mask)
        round_img.save(f"{m}/ic_launcher_round.png")

        fg.save(f"{m}/ic_launcher_foreground.png")
        bg.save(f"{m}/ic_launcher_background.png")
        print(f"✓ mipmap-{dpi}")


if __name__ == "__main__":
    main()
