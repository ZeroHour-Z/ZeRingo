# dmgbuild 配置。由 bundle.sh 调用，通过 -D 传入 app、bg、icon。
# 图标坐标需与 make_dmg_bg.py 中的 APP / APPS 一致。
import os

app = defines["app"]
app_name = os.path.basename(app)

format = "UDZO"
filesystem = "HFS+"
files = [app]
symlinks = {"应用程序": "/Applications"}
icon = defines["icon"]          # 卷图标
background = defines["bg"]

window_rect = ((200, 120), (660, 420))
default_view = "icon-view"
show_status_bar = False
show_tab_view = False
show_toolbar = False
show_pathbar = False
show_sidebar = False
show_icon_preview = False
arrange_by = None
icon_size = 112
text_size = 13
label_pos = "bottom"

icon_locations = {
    app_name: (180, 230),
    "应用程序": (480, 230),
    # 开启“显示隐藏文件”时，把隐藏项放到窗口外
    ".background.tiff": (180, 900),
    ".VolumeIcon.icns": (330, 900),
    ".fseventsd": (480, 900),
    ".DS_Store": (630, 900),
    ".Trashes": (780, 900),
}
