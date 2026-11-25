//! ZeRingo：NCM 格式转换器（macOS）。
//!
//! 常驻菜单栏。按住 ⌥ Option 键拖动 .ncm 文件时，光标处会出现一个圆环：
//! 拖进左半边转 MP3，拖进右半边转 WAV，输出到源文件同目录。
//!
//! 也可以命令行使用：`zeringo mp3|wav 文件...`

mod convert;
mod ncm;

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use convert::Target;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, ProtocolObject};
use objc2::{AllocAnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSBezierPath, NSColor,
    NSDragOperation, NSDraggingDestination, NSDraggingInfo, NSEvent, NSEventModifierFlags, NSFont,
    NSFontAttributeName, NSFontDescriptorSystemDesignRounded, NSFontWeightBold,
    NSFontWeightSemibold, NSForegroundColorAttributeName, NSGradient, NSGraphicsContext, NSShadow,
    NSAnimatablePropertyContainer, NSControlStateValueOff, NSControlStateValueOn, NSMenu, NSMenuItem, NSPasteboard,
    NSPasteboardNameDrag, NSPasteboardTypeFileURL, NSStatusBar, NSStatusItem,
    NSStringDrawing, NSVariableStatusItemLength, NSView, NSWindow, NSWindowCollectionBehavior,
    NSWindowStyleMask,
};
use objc2_service_management::{SMAppService, SMAppServiceStatus};
use objc2_foundation::{
    NSArray, NSBundle, NSDictionary, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, NSTimer, NSURL,
};

/// 触发圆环的按键。可改成 Control / Command / Shift。
const HOTKEY: NSEventModifierFlags = NSEventModifierFlags::Option;
/// 圆环外半径、内半径（中心无效区）、悬停时放大量、留给阴影的边距（点）。
const RADIUS: f64 = 64.0;
const INNER: f64 = 26.0;
const GROW: f64 = 5.0;
const PAD: f64 = 14.0;
/// 左右两半之间的缝宽（点）。
const GAP: f64 = 5.0;
/// 窗口半边长，圆心在窗口中心。
const HALF: f64 = RADIUS + GROW + PAD;

static ACTIVE: AtomicUsize = AtomicUsize::new(0);
static LAST: Mutex<Option<Result<String, String>>> = Mutex::new(None);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    None,
    Left,
    Right,
}

fn ncm_files(pb: &NSPasteboard) -> Vec<PathBuf> {
    let Some(items) = pb.pasteboardItems() else {
        return vec![];
    };
    items
        .iter()
        .filter_map(|item| {
            let s = item.stringForType(unsafe { NSPasteboardTypeFileURL })?;
            // Finder 给的可能是 file reference URL（file:///.file/id=…），需要解析成路径
            let url = NSURL::URLWithString(&s)?.filePathURL()?;
            let p = PathBuf::from(url.path()?.to_string());
            p.extension()?.eq_ignore_ascii_case("ncm").then_some(p)
        })
        .collect()
}

fn start_conversion(files: Vec<PathBuf>, target: Target) {
    ACTIVE.fetch_add(files.len(), Ordering::SeqCst);
    std::thread::spawn(move || {
        for f in files {
            let r = convert::convert(&f, target);
            match &r {
                Ok(out) => eprintln!("✓ {} -> {}", f.display(), out.display()),
                Err(e) => eprintln!("✗ {}: {e}", f.display()),
            }
            let name = f.file_name().unwrap_or_default().to_string_lossy().into_owned();
            *LAST.lock().unwrap() = Some(match r {
                Ok(out) => Ok(format!("已转换：{}", out.display())),
                Err(e) => Err(format!("{name} 转换失败：{e}")),
            });
            ACTIVE.fetch_sub(1, Ordering::SeqCst);
        }
    });
}

// ---------- 圆环视图 ----------

struct RingIvars {
    hover: Cell<Side>,
}

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "ZeRingoRingView"]
    #[ivars = RingIvars]
    struct RingView;

    impl RingView {
        #[unsafe(method(isOpaque))]
        fn is_opaque(&self) -> bool {
            false
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            self.draw();
        }
    }

    unsafe impl NSObjectProtocol for RingView {}

    unsafe impl NSDraggingDestination for RingView {
        #[unsafe(method(draggingEntered:))]
        fn dragging_entered(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            self.update_hover(sender)
        }

        #[unsafe(method(draggingUpdated:))]
        fn dragging_updated(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            self.update_hover(sender)
        }

        #[unsafe(method(draggingExited:))]
        fn dragging_exited(&self, _sender: Option<&ProtocolObject<dyn NSDraggingInfo>>) {
            self.set_hover(Side::None);
        }

        #[unsafe(method(performDragOperation:))]
        fn perform_drag(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> bool {
            self.handle_drop(sender)
        }
    }
);

impl RingView {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(HALF * 2.0, HALF * 2.0));
        let this = Self::alloc(mtm).set_ivars(RingIvars { hover: Cell::new(Side::None) });
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        this.registerForDraggedTypes(&NSArray::from_slice(&[unsafe { NSPasteboardTypeFileURL }]));
        this
    }

    fn handle_drop(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> bool {
        let side = self.side_at(sender);
        self.set_hover(Side::None);
        let target = match side {
            Side::Left => Target::Mp3,
            Side::Right => Target::Wav,
            Side::None => return false,
        };
        let files = ncm_files(&sender.draggingPasteboard());
        if files.is_empty() {
            return false;
        }
        start_conversion(files, target);
        if let Some(w) = self.window() {
            w.orderOut(None);
        }
        true
    }

    fn side_at(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> Side {
        let p = self.convertPoint_fromView(sender.draggingLocation(), None);
        let (dx, dy) = (p.x - HALF, p.y - HALF);
        let d = dx.hypot(dy);
        if d > RADIUS + GROW + PAD / 2.0 || d < INNER {
            Side::None
        } else if dx < 0.0 {
            Side::Left
        } else {
            Side::Right
        }
    }

    fn update_hover(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
        let side = self.side_at(sender);
        self.set_hover(side);
        if side == Side::None || ncm_files(&sender.draggingPasteboard()).is_empty() {
            NSDragOperation::None
        } else {
            NSDragOperation::Copy
        }
    }

    fn set_hover(&self, side: Side) {
        if self.ivars().hover.replace(side) != side {
            self.setNeedsDisplay(true);
        }
    }

    fn draw(&self) {
        let hover = self.ivars().hover.get();
        let c = NSPoint::new(HALF, HALF);
        let rgba = |r: f64, g: f64, b: f64, a: f64| NSColor::colorWithSRGBRed_green_blue_alpha(r, g, b, a);
        let rounded = |size: f64, weight: f64| {
            let base = NSFont::systemFontOfSize_weight(size, weight);
            base.fontDescriptor()
                .fontDescriptorWithDesign(unsafe { NSFontDescriptorSystemDesignRounded })
                .and_then(|d| NSFont::fontWithDescriptor_size(&d, size))
                .unwrap_or(base)
        };
        let label = |text: &str, font: &NSFont, color: &NSColor, x: f64, y: f64| {
            let s = NSString::from_str(text);
            let attrs: Retained<NSDictionary<NSString, AnyObject>> = NSDictionary::from_slices(
                &[unsafe { NSFontAttributeName }, unsafe { NSForegroundColorAttributeName }],
                &[font as &AnyObject, color as &AnyObject],
            );
            let size = unsafe { s.sizeWithAttributes(Some(&attrs)) };
            let at = NSPoint::new(x - size.width / 2.0, y - size.height / 2.0);
            unsafe { s.drawAtPoint_withAttributes(at, Some(&attrs)) };
        };
        // 圆环的一段：外弧逆时针，内弧顺时针返回。两侧各让出 GAP/2 的等宽缝。
        let segment = |left: bool, outer: f64| {
            let inner = INNER + 4.0;
            let ao = (GAP / 2.0 / outer).asin().to_degrees();
            let ai = (GAP / 2.0 / inner).asin().to_degrees();
            let (os, oe, is, ie) = if left {
                (90.0 + ao, 270.0 - ao, 270.0 - ai, 90.0 + ai)
            } else {
                (-90.0 + ao, 90.0 - ao, 90.0 - ai, -90.0 + ai)
            };
            let p = NSBezierPath::bezierPath();
            p.appendBezierPathWithArcWithCenter_radius_startAngle_endAngle(c, outer, os, oe);
            p.appendBezierPathWithArcWithCenter_radius_startAngle_endAngle_clockwise(c, inner, is, ie, true);
            p.closePath();
            p
        };
        let shadow = |blur: f64, alpha: f64| {
            let sh = NSShadow::new();
            sh.setShadowBlurRadius(blur);
            sh.setShadowOffset(NSSize::new(0.0, -3.0));
            sh.setShadowColor(Some(&rgba(0.0, 0.0, 0.0, alpha)));
            sh.set();
        };

        // 左：MP3（蓝紫渐变）；右：WAV（青绿渐变）。两段之间留 4° 缝。
        let halves = [
            (true, Side::Left, "MP3", (0.36, 0.55, 1.0), (0.55, 0.36, 0.98), -1.0),
            (false, Side::Right, "WAV", (0.13, 0.80, 0.72), (0.20, 0.78, 0.40), 1.0),
        ];
        let label_font = rounded(13.0, unsafe { NSFontWeightBold });
        for (is_left, side, text, c1, c2, dir) in halves {
            let active = hover == side;
            let dim = hover != Side::None && !active;
            let alpha = if dim { 0.55 } else { 1.0 };
            let outer = if active { RADIUS + GROW } else { RADIUS };
            let path = segment(is_left, outer);

            NSGraphicsContext::saveGraphicsState_class();
            shadow(if active { 14.0 } else { 9.0 }, 0.35);
            rgba(c1.0, c1.1, c1.2, alpha).setFill();
            path.fill();
            NSGraphicsContext::restoreGraphicsState_class();

            if let Some(g) = NSGradient::initWithStartingColor_endingColor(
                NSGradient::alloc(),
                &rgba(c1.0, c1.1, c1.2, alpha),
                &rgba(c2.0, c2.1, c2.2, alpha),
            ) {
                g.drawInBezierPath_angle(&path, -90.0);
            }
            // 顶部高光，增加一点质感
            if let Some(g) = NSGradient::initWithStartingColor_endingColor(
                NSGradient::alloc(),
                &rgba(1.0, 1.0, 1.0, if active { 0.30 } else { 0.18 }),
                &rgba(1.0, 1.0, 1.0, 0.0),
            ) {
                g.drawInBezierPath_angle(&path, -90.0);
            }
            rgba(1.0, 1.0, 1.0, if active { 0.6 } else { 0.25 }).setStroke();
            path.setLineWidth(1.0);
            path.stroke();

            let lx = HALF + dir * (INNER + 4.0 + outer) / 2.0;
            label(text, &label_font, &rgba(1.0, 1.0, 1.0, if dim { 0.7 } else { 1.0 }), lx, HALF);
        }

        // 中心圆：深色半透明，显示当前选择
        let hole = NSBezierPath::bezierPathWithOvalInRect(NSRect::new(
            NSPoint::new(HALF - INNER, HALF - INNER),
            NSSize::new(INNER * 2.0, INNER * 2.0),
        ));
        NSGraphicsContext::saveGraphicsState_class();
        shadow(8.0, 0.3);
        rgba(0.11, 0.11, 0.13, 0.88).setFill();
        hole.fill();
        NSGraphicsContext::restoreGraphicsState_class();
        rgba(1.0, 1.0, 1.0, 0.25).setStroke();
        hole.setLineWidth(1.0);
        hole.stroke();
        let (center_text, size) = match hover {
            Side::Left => ("←", 18.0),
            Side::Right => ("→", 18.0),
            Side::None => ("♪", 18.0),
        };
        label(center_text, &rounded(size, unsafe { NSFontWeightSemibold }), &rgba(1.0, 1.0, 1.0, 0.92), HALF, HALF);
    }
}

// ---------- 控制器：轮询按键和拖拽状态 ----------

struct ControllerIvars {
    window: Retained<NSWindow>,
    status: Retained<NSStatusItem>,
    visible: Cell<bool>,
    /// 鼠标松开时拖拽剪贴板的 changeCount；按下后它变化说明开始了新的拖拽
    baseline: Cell<isize>,
    /// 已检查过是否含 .ncm 的 changeCount，避免每帧重复读剪贴板
    checked: Cell<isize>,
    title: RefCell<String>,
    last_item: Retained<NSMenuItem>,
    login_item: Retained<NSMenuItem>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ZeRingoController"]
    #[ivars = ControllerIvars]
    struct Controller;

    impl Controller {
        #[unsafe(method(tick:))]
        fn tick(&self, _timer: &NSTimer) {
            self.poll();
            self.refresh_status();
        }

        #[unsafe(method(toggleLogin:))]
        fn toggle_login(&self, _sender: &NSMenuItem) {
            let svc = unsafe { SMAppService::mainAppService() };
            let enabled = unsafe { svc.status() } == SMAppServiceStatus::Enabled;
            let r = unsafe {
                if enabled { svc.unregisterAndReturnError() } else { svc.registerAndReturnError() }
            };
            if let Err(e) = r {
                *LAST.lock().unwrap() = Some(Err(format!("设置开机启动失败：{}", e.localizedDescription())));
            }
            self.refresh_login();
        }
    }

    unsafe impl NSObjectProtocol for Controller {}
);

impl Controller {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let size = NSSize::new(HALF * 2.0, HALF * 2.0);
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                NSRect::new(NSPoint::new(0.0, 0.0), size),
                NSWindowStyleMask::Borderless,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe { window.setReleasedWhenClosed(false) };
        window.setOpaque(false);
        window.setBackgroundColor(Some(&NSColor::clearColor()));
        window.setHasShadow(false);
        window.setLevel(101); // NSPopUpMenuWindowLevel，盖在普通窗口之上
        window.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                | NSWindowCollectionBehavior::Stationary,
        );
        window.setContentView(Some(&RingView::new(mtm)));

        let status = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
        let menu = NSMenu::new(mtm);
        let text_item = |t: &str| {
            let it = NSMenuItem::new(mtm);
            it.setTitle(&NSString::from_str(t));
            it.setEnabled(false);
            it
        };
        menu.addItem(&text_item("ZeRingo"));
        menu.addItem(&text_item("按住 ⌥ 拖动 .ncm：左 MP3 / 右 WAV"));
        let last_item = text_item("还没有转换记录");
        menu.addItem(&last_item);
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        let login_item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str("登录时自动启动"),
                Some(sel!(toggleLogin:)),
                &NSString::from_str(""),
            )
        };
        menu.addItem(&login_item);
        let quit = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str("退出 ZeRingo"),
                Some(sel!(terminate:)),
                &NSString::from_str("q"),
            )
        };
        menu.addItem(&quit);
        status.setMenu(Some(&menu));

        let cc = Self::drag_pasteboard().changeCount();
        let this = Self::alloc(mtm).set_ivars(ControllerIvars {
            window,
            status,
            visible: Cell::new(false),
            baseline: Cell::new(cc),
            checked: Cell::new(cc),
            title: RefCell::new(String::new()),
            last_item,
            login_item,
        });
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };
        unsafe { this.ivars().login_item.setTarget(Some(&this)) };
        this.refresh_login();
        this.refresh_status();
        this
    }

    fn drag_pasteboard() -> Retained<NSPasteboard> {
        NSPasteboard::pasteboardWithName(unsafe { NSPasteboardNameDrag })
    }

    fn poll(&self) {
        let iv = self.ivars();
        let pb = Self::drag_pasteboard();
        let cc = pb.changeCount();
        let mouse_down = NSEvent::pressedMouseButtons() & 1 != 0;

        if !mouse_down {
            iv.baseline.set(cc);
            if iv.visible.replace(false) {
                iv.window.orderOut(None);
            }
            return;
        }
        if iv.visible.get() || cc == iv.baseline.get() || cc == iv.checked.get() {
            return;
        }
        if !NSEvent::modifierFlags_class().contains(HOTKEY) {
            return;
        }
        // 正在拖拽且按着快捷键：确认拖的是 .ncm 再显示
        iv.checked.set(cc);
        if ncm_files(&pb).is_empty() {
            return;
        }
        let m = NSEvent::mouseLocation();
        let origin = NSPoint::new(m.x - HALF, m.y - HALF);
        iv.window.setFrameOrigin(origin);
        iv.window.setAlphaValue(0.0);
        iv.window.orderFrontRegardless();
        iv.window.animator().setAlphaValue(1.0);
        iv.visible.set(true);
    }

    fn refresh_status(&self) {
        let iv = self.ivars();
        let active = ACTIVE.load(Ordering::SeqCst);
        let last = LAST.lock().unwrap().clone();
        let (title, tip) = match (&last, active) {
            (_, n) if n > 0 => (format!("◐ ⏳{n}"), format!("正在转换 {n} 个文件…")),
            (Some(Ok(m)), _) => ("◐ ✓".into(), m.clone()),
            (Some(Err(m)), _) => ("◐ ✗".into(), m.clone()),
            (None, _) => ("◐".into(), "ZeRingo：按住 ⌥ 拖动 .ncm 文件".into()),
        };
        if *iv.title.borrow() == format!("{title}{tip}") {
            return;
        }
        *iv.title.borrow_mut() = format!("{title}{tip}");
        if let Some(button) = iv.status.button(self.mtm()) {
            button.setTitle(&NSString::from_str(&title));
            button.setToolTip(Some(&NSString::from_str(&tip)));
        }
        if last.is_some() || active > 0 {
            iv.last_item.setTitle(&NSString::from_str(&tip));
        }
    }

    /// 只有从 .app 启动时才能设置登录项。
    fn refresh_login(&self) {
        let item = &self.ivars().login_item;
        let in_bundle = NSBundle::mainBundle().bundlePath().to_string().ends_with(".app");
        item.setEnabled(in_bundle);
        let on = in_bundle && unsafe { SMAppService::mainAppService().status() } == SMAppServiceStatus::Enabled;
        item.setState(if on { NSControlStateValueOn } else { NSControlStateValueOff });
    }
}

fn run_cli(args: &[String]) -> i32 {
    let target = match args[0].as_str() {
        "mp3" => Target::Mp3,
        "wav" => Target::Wav,
        _ => {
            eprintln!("用法：zeringo [mp3|wav 文件...]\n不带参数运行则启动拖拽模式。");
            return 2;
        }
    };
    let mut code = 0;
    for f in &args[1..] {
        match convert::convert(f.as_ref(), target) {
            Ok(out) => println!("✓ {f} -> {}", out.display()),
            Err(e) => {
                eprintln!("✗ {f}: {e}");
                code = 1;
            }
        }
    }
    code
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.is_empty() {
        std::process::exit(run_cli(&args));
    }

    let mtm = MainThreadMarker::new().expect("必须在主线程运行");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    let controller = Controller::new(mtm);
    let _timer = unsafe {
        NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
            1.0 / 60.0,
            &controller,
            sel!(tick:),
            None,
            true,
        )
    };
    eprintln!("ZeRingo 已启动：按住 ⌥ Option 拖动 .ncm 文件，左半边 MP3，右半边 WAV。");
    app.run();
}
