"""生成 ZeRingo 图标（1024x1024 PNG）。用法：python3 make_icon.py 输出.png"""
import math, sys
from PIL import Image, ImageDraw, ImageFilter

S = 1024
SS = 4  # 超采样抗锯齿
W = S * SS

def lerp(a, b, t):
    return tuple(int(a[i] + (b[i] - a[i]) * t) for i in range(len(a)))

def gradient(size, top, bottom):
    g = Image.new("RGBA", (1, size))
    for y in range(size):
        g.putpixel((0, y), lerp(top, bottom, y / (size - 1)))
    return g.resize((size, size))

img = Image.new("RGBA", (W, W), (0, 0, 0, 0))

# macOS 风格圆角方形底板（深色）
m = int(W * 0.098)
bg_mask = Image.new("L", (W, W), 0)
ImageDraw.Draw(bg_mask).rounded_rectangle([m, m, W - m, W - m], radius=int(W * 0.185), fill=255)
shadow = Image.new("RGBA", (W, W), (0, 0, 0, 0))
shadow.paste((0, 0, 0, 110), (0, int(W * 0.012)), bg_mask)
img = Image.alpha_composite(img, shadow.filter(ImageFilter.GaussianBlur(W * 0.012)))
img.paste(gradient(W, (44, 46, 58, 255), (20, 20, 26, 255)), (0, 0), bg_mask)

c = W / 2
R = W * 0.33       # 外半径
r = W * 0.135      # 内半径
gap = W * 0.022    # 左右缝宽

def half_mask(left):
    mk = Image.new("L", (W, W), 0)
    d = ImageDraw.Draw(mk)
    d.ellipse([c - R, c - R, c + R, c + R], fill=255)
    d.ellipse([c - r, c - r, c + r, c + r], fill=0)
    if left:
        d.rectangle([c - gap / 2, 0, W, W], fill=0)
    else:
        d.rectangle([0, 0, c + gap / 2, W], fill=0)
    return mk

ring_shadow = Image.new("RGBA", (W, W), (0, 0, 0, 0))
for left in (True, False):
    ring_shadow.paste((0, 0, 0, 140), (0, int(W * 0.01)), half_mask(left))
img = Image.alpha_composite(img, ring_shadow.filter(ImageFilter.GaussianBlur(W * 0.018)))

for left, top, bottom in ((True, (92, 140, 255), (140, 92, 250)), (False, (33, 204, 184), (51, 199, 102))):
    mk = half_mask(left)
    img.paste(gradient(W, top + (255,), bottom + (255,)), (0, 0), mk)
    # 顶部高光
    hl = gradient(W, (255, 255, 255, 80), (255, 255, 255, 0))
    hl_mask = Image.new("L", (W, W), 0)
    hl_mask.paste(mk, (0, 0))
    layer = Image.new("RGBA", (W, W), (0, 0, 0, 0))
    layer.paste(hl, (0, 0), hl_mask)
    img = Image.alpha_composite(img, layer)

# 中心深色圆 + 细描边
d = ImageDraw.Draw(img)
d.ellipse([c - r + gap, c - r + gap, c + r - gap, c + r - gap], fill=(28, 28, 33, 255),
          outline=(255, 255, 255, 70), width=int(W * 0.004))

# 中心的“Z”（ZeroHour）
zw = r * 0.78
zt = W * 0.026
x0, x1, y0, y1 = c - zw / 2, c + zw / 2, c - zw / 2, c + zw / 2
for seg in (((x0, y0), (x1, y0)), ((x1, y0), (x0, y1)), ((x0, y1), (x1, y1))):
    d.line(seg, fill=(255, 255, 255, 240), width=int(zt))
for p in ((x0, y0), (x1, y0), (x0, y1), (x1, y1)):
    d.ellipse([p[0] - zt / 2, p[1] - zt / 2, p[0] + zt / 2, p[1] + zt / 2], fill=(255, 255, 255, 240))

img.resize((S, S), Image.LANCZOS).save(sys.argv[1])
