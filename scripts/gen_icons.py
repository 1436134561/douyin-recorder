from PIL import Image, ImageDraw, ImageFilter

SIZE = 1024
img = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
d = ImageDraw.Draw(img)

# 圆角背景渐变（用两层近似）
def rounded_rect(draw, box, r, fill):
    draw.rounded_rectangle(box, radius=r, fill=fill)

# 背景：品牌蓝渐变（简单双色叠加）
bg = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
bd = ImageDraw.Draw(bg)
bd.rounded_rectangle([0, 0, SIZE, SIZE], radius=224, fill=(37, 99, 235, 255))
# 叠加浅色高光
hl = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
hd = ImageDraw.Draw(hl)
hd.rounded_rectangle([0, 0, SIZE, SIZE // 2], radius=224, fill=(59, 130, 246, 120))
bg = Image.alpha_composite(bg, hl)
img = bg

# 白色录制圆点
dd = ImageDraw.Draw(img)
cx, cy = SIZE // 2, SIZE // 2
r = 150
dd.ellipse([cx - r, cy - r, cx + r, cy + r], fill=(255, 255, 255, 255))
# 内圈细微描边感
dd.ellipse([cx - r + 14, cy - r + 14, cx + r - 14, cy + r - 14], fill=(37, 99, 235, 255))

def save_variants(base):
    base.save("src-tauri/icons/icon.png")
    base.resize((32, 32)).save("src-tauri/icons/32x32.png")
    base.resize((128, 128)).save("src-tauri/icons/128x128.png")
    base.resize((256, 256)).save("src-taurin/icons/128x128@2x.png") if False else None
    base.resize((256, 256)).save("src-tauri/icons/128x128@2x.png")
    # ico 多尺寸
    base.save("src-tauri/icons/icon.ico", sizes=[(256, 256), (128, 128), (64, 64), (32, 32), (16, 16)])
    # icns
    base.save("src-tauri/icons/icon.icns")

save_variants(img)
print("icons generated")
