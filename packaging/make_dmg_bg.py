"""生成 DMG 窗口背景。用法：python3 make_dmg_bg.py 输出目录
输出 bg.png (660x420) 和 bg@2x.png；图标位置需与 bundle.sh 中的坐标一致：
App 中心 (180, 230)，应用程序 中心 (480, 230)。"""
import sys
from PIL import Image, ImageDraw, ImageFont, ImageFilter

W, H = 660, 420
APP, APPS = (180, 230), (480, 230)
FONT = "/System/Library/Fonts/Hiragino Sans GB.ttc"

def lerp(a, b, t):
    return tuple(round(a[i] + (b[i] - a[i]) * t) for i in range(len(a)))

def render(k):
    w, h = W * k, H * k
    img = Image.new("RGB", (w, h))
    top, bot = (246, 247, 251), (230, 233, 242)
    for y in range(h):
        ImageDraw.Draw(img).line([(0, y), (w, y)], fill=lerp(top, bot, y / (h - 1)))
    img = img.convert("RGBA")

    # 两个淡色光晕，呼应圆环的蓝紫/青绿
    # 底色用同色透明，避免模糊时混入黑色发灰
    for (cx, cy), col in ((APP, (120, 110, 255)), (APPS, (40, 200, 150))):
        glow = Image.new("RGBA", (w, h), col + (0,))
        r = 110 * k
        ImageDraw.Draw(glow).ellipse([cx * k - r, cy * k - r, cx * k + r, cy * k + r], fill=col + (60,))
        img = Image.alpha_composite(img, glow.filter(ImageFilter.GaussianBlur(55 * k)))

    d = ImageDraw.Draw(img)
    title = ImageFont.truetype("/System/Library/Fonts/SFNSRounded.ttf", 30 * k)
    try:
        title.set_variation_by_name("Bold")
    except Exception:
        pass
    sub = ImageFont.truetype(FONT, 13 * k, index=0)
    def center_text(text, font, y, fill):
        tw = d.textlength(text, font=font)
        d.text(((w - tw) / 2, y * k), text, font=font, fill=fill)
    center_text("ZeRingo", title, 34, (40, 42, 54))
    center_text("把 ZeRingo 拖到「应用程序」即可完成安装", sub, 82, (110, 114, 130))

    # 渐变箭头：从蓝紫到青绿
    x0, x1, y = (APP[0] + 70) * k, (APPS[0] - 70) * k, APP[1] * k
    t = 4 * k
    arrow = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    a = ImageDraw.Draw(arrow)
    steps = int(x1 - x0)
    for i in range(steps):
        col = lerp((120, 110, 250, 255), (45, 195, 140, 255), i / steps)
        a.line([(x0 + i, y - t / 2), (x0 + i, y + t / 2)], fill=col)
    head = 11 * k
    a.polygon([(x1 + head * 0.4, y), (x1 - head, y - head), (x1 - head, y + head)], fill=(45, 195, 140, 255))
    for cx in (x0,):
        a.ellipse([cx - t / 2, y - t / 2, cx + t / 2, y + t / 2], fill=(120, 110, 250, 255))
    img = Image.alpha_composite(img, arrow)
    d = ImageDraw.Draw(img)

    foot = ImageFont.truetype(FONT, 11 * k, index=0)
    center_text("首次打开若被拦截：系统设置 → 隐私与安全性 → 仍要打开", foot, 336, (140, 144, 160))
    return img.convert("RGB")

out = sys.argv[1]
render(1).save(f"{out}/bg.png")
render(2).save(f"{out}/bg@2x.png")
