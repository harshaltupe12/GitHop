//! System tray: runtime-rendered icon, tooltip, native right-click menu and
//! the small frameless tray menu window (Design §6.2, §7.14).
//!
//! INTEGRATE (core `check`): when a commit/push is blocked, write the current
//! unix time in seconds (ASCII) to `<data dir>/last-block`
//! ([`model::LAST_BLOCK_FILE`]) — e.g. in `check::run_check` when the outcome is
//! a block. Best effort, never fail the hook on it. The file watcher picks it
//! up and the tray shows the vermillion dot for 10 minutes.

pub mod model;
pub mod render;

use std::sync::Mutex;
use std::time::{Duration, Instant};

use githop_core::{hooks, Git, Paths, Store};
use tauri::image::Image;
use tauri::menu::{CheckMenuItem, IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{
    AppHandle, Manager, PhysicalPosition, PhysicalSize, Rect, Runtime, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder, WindowEvent,
};

use model::TrayModel;

/// Tray icon id.
pub const TRAY_ID: &str = "githop";
/// Label of the frameless tray menu window (UI route `index.html#tray`).
pub const MENU_WINDOW: &str = "tray";
/// Design §7.14: width 260, items 32 px.
const MENU_WIDTH: f64 = 260.0;
const MENU_ITEM: f64 = 32.0;
/// Header ("Default profile") + separator + two actions + padding.
const MENU_CHROME: f64 = 8.0 + 28.0 + 9.0 + 2.0 * MENU_ITEM + 8.0;
const MENU_MAX_HEIGHT: f64 = 560.0;
/// Gap between the tray menu window and the taskbar/screen edge.
const MENU_MARGIN: f64 = 8.0;
/// A click this soon after the menu window closed on blur is the same click
/// (on the tray icon) — don't reopen it.
const REOPEN_GUARD: Duration = Duration::from_millis(400);
/// How often the "blocked recently" dot is re-checked for expiry.
const BLOCKED_POLL: Duration = Duration::from_secs(30);

/// Menu item ids.
const ID_OPEN: &str = "open";
const ID_QUIT: &str = "quit";
const ID_PROFILE_PREFIX: &str = "profile:";

/// Last model shown (so the blocked-dot timer can re-render without git).
#[derive(Default)]
struct TrayCache {
    model: Option<TrayModel>,
    blocked: bool,
    menu_hidden_at: Option<Instant>,
}

type Cache = Mutex<TrayCache>;

/// Create the tray icon and start the blocked-dot timer. Call from `setup`.
pub fn init<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    app.manage(Cache::default());
    let initial = model::model(Err(String::new()), None);
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("GitHop")
        .show_menu_on_left_click(false)
        .on_menu_event(on_menu_event)
        .on_tray_icon_event(on_tray_event);
    let size = icon_px(app);
    builder = builder.icon(Image::new_owned(
        render::render(&model::icon_spec(&initial, false), size),
        size,
        size,
    ));
    builder = builder.menu(&build_menu(app, None)?);
    builder.build(app)?;
    refresh(app);
    start_blocked_timer(app.clone());
    Ok(())
}

/// Recompute everything (reads data + one git spawn) on a worker thread, then
/// update icon, tooltip and menu. Safe to call from any thread.
pub fn refresh<R: Runtime>(app: &AppHandle<R>) {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let store = Store::open();
        let set = store
            .as_ref()
            .map(|s| s.profiles().clone())
            .map_err(|e| e.to_string());
        let health = match (&store, Git::locate()) {
            (Ok(s), Ok(git)) => Some(hooks::verify_hooks(s.paths(), &git, s.settings())),
            _ => None,
        };
        let m = model::model(set.as_ref().map_err(Clone::clone), health.as_ref());
        let blocked = blocked_now();
        if let Some(cache) = app.try_state::<Cache>() {
            if let Ok(mut c) = cache.lock() {
                c.model = Some(m.clone());
                c.blocked = blocked;
            }
        }
        apply(&app, &m, blocked, set.ok().as_ref());
    });
}

fn blocked_now() -> bool {
    Paths::resolve()
        .ok()
        .is_some_and(|p| model::recently_blocked(model::last_block(&p), model::now_secs()))
}

/// Push icon, tooltip and (when `set` is given) the native menu to the tray.
fn apply<R: Runtime>(
    app: &AppHandle<R>,
    m: &TrayModel,
    blocked: bool,
    set: Option<&githop_core::ProfileSet>,
) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };
    let size = icon_px(app);
    let rgba = render::render(&model::icon_spec(m, blocked), size);
    log_err(
        "tray icon",
        tray.set_icon(Some(Image::new_owned(rgba, size, size))),
    );
    log_err(
        "tray tooltip",
        tray.set_tooltip(Some(model::tooltip(m, blocked))),
    );
    if let Some(set) = set {
        match build_menu(app, Some(set)) {
            Ok(menu) => log_err("tray menu", tray.set_menu(Some(menu))),
            Err(e) => eprintln!("GitHop: couldn't build the tray menu: {e}"),
        }
    }
}

fn log_err<E: std::fmt::Display>(what: &str, r: Result<(), E>) {
    if let Err(e) = r {
        eprintln!("GitHop: couldn't update the {what}: {e}");
    }
}

/// Re-check the blocked dot periodically so it disappears after 10 minutes.
fn start_blocked_timer<R: Runtime>(app: AppHandle<R>) {
    std::thread::spawn(move || loop {
        std::thread::sleep(BLOCKED_POLL);
        let blocked = blocked_now();
        let Some(cache) = app.try_state::<Cache>() else {
            continue;
        };
        let changed = match cache.lock() {
            Ok(mut c) if c.blocked != blocked => {
                c.blocked = blocked;
                c.model.clone()
            }
            _ => None,
        };
        if let Some(m) = changed {
            apply(&app, &m, blocked, None);
        }
    });
}

/// Icon edge in pixels for the primary monitor's scale.
fn icon_px<R: Runtime>(app: &AppHandle<R>) -> u32 {
    let scale = app
        .primary_monitor()
        .ok()
        .flatten()
        .map_or(1.0, |m| m.scale_factor());
    render::tray_px(scale)
}

/// Native right-click menu: "Default profile" header, each profile (checked =
/// default; click = make default), Open GitHop, Quit.
fn build_menu<R: Runtime>(
    app: &AppHandle<R>,
    set: Option<&githop_core::ProfileSet>,
) -> tauri::Result<Menu<R>> {
    let mut items: Vec<Box<dyn IsMenuItem<R>>> = Vec::new();
    items.push(Box::new(MenuItem::with_id(
        app,
        "header",
        "Default profile",
        false,
        None::<&str>,
    )?));
    match set {
        Some(set) if !set.profiles.is_empty() => {
            for p in &set.profiles {
                let checked = set.default.as_deref() == Some(p.id.as_str());
                items.push(Box::new(CheckMenuItem::with_id(
                    app,
                    format!("{ID_PROFILE_PREFIX}{}", p.id),
                    menu_text(&p.label, &p.email),
                    true,
                    checked,
                    None::<&str>,
                )?));
            }
        }
        _ => items.push(Box::new(MenuItem::with_id(
            app,
            "none",
            "No profiles yet",
            false,
            None::<&str>,
        )?)),
    }
    items.push(Box::new(PredefinedMenuItem::separator(app)?));
    items.push(Box::new(MenuItem::with_id(
        app,
        ID_OPEN,
        "Open GitHop",
        true,
        None::<&str>,
    )?));
    items.push(Box::new(MenuItem::with_id(
        app,
        ID_QUIT,
        "Quit",
        true,
        None::<&str>,
    )?));
    let refs: Vec<&dyn IsMenuItem<R>> = items.iter().map(|i| i.as_ref()).collect();
    Menu::with_items(app, &refs)
}

/// `Office   harshal@company.com` — `&` escaped (it marks accelerators).
fn menu_text(label: &str, email: &str) -> String {
    format!("{}\t{}", label.replace('&', "&&"), email.replace('&', "&&"))
}

fn on_menu_event<R: Runtime>(app: &AppHandle<R>, event: MenuEvent) {
    let id = event.id().as_ref();
    match id {
        ID_OPEN => crate::show_main_window(app),
        ID_QUIT => app.exit(0),
        _ => {
            if let Some(profile) = id.strip_prefix(ID_PROFILE_PREFIX) {
                switch_from_tray(app, profile.to_string());
            }
        }
    }
}

/// Make `id` the default profile from the tray, off the UI thread.
fn switch_from_tray<R: Runtime>(app: &AppHandle<R>, id: String) {
    if crate::validate::profile_id(&id).is_err() {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        match crate::commands::profiles::set_default_blocking(&id) {
            Ok(set) => crate::events::default_changed(&app, &set),
            Err(e) => {
                eprintln!("GitHop: couldn't switch the default profile: {}", e.message);
                // Re-sync the check marks with what's really on disk.
                refresh(&app);
            }
        }
    });
}

fn on_tray_event(tray: &TrayIcon<impl Runtime>, event: TrayIconEvent) {
    match event {
        TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            rect,
            ..
        } => toggle_menu_window(tray.app_handle(), rect),
        TrayIconEvent::DoubleClick {
            button: MouseButton::Left,
            ..
        } => {
            hide_menu_window(tray.app_handle());
            crate::show_main_window(tray.app_handle());
        }
        _ => {}
    }
}

/// Hide the tray menu window (blur, Esc, or after an action).
pub fn hide_menu_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(w) = app.get_webview_window(MENU_WINDOW) {
        if w.is_visible().unwrap_or(false) {
            let _ = w.hide();
            mark_menu_hidden(app);
        }
    }
}

fn mark_menu_hidden<R: Runtime>(app: &AppHandle<R>) {
    if let Some(cache) = app.try_state::<Cache>() {
        if let Ok(mut c) = cache.lock() {
            c.menu_hidden_at = Some(Instant::now());
        }
    }
}

fn hidden_just_now<R: Runtime>(app: &AppHandle<R>) -> bool {
    app.try_state::<Cache>()
        .and_then(|cache| cache.lock().ok().and_then(|c| c.menu_hidden_at))
        .is_some_and(|t| t.elapsed() < REOPEN_GUARD)
}

/// Left click: open the tray menu window next to the icon, or close it.
fn toggle_menu_window<R: Runtime>(app: &AppHandle<R>, icon: Rect) {
    if let Some(w) = app.get_webview_window(MENU_WINDOW) {
        if w.is_visible().unwrap_or(false) {
            hide_menu_window(app);
            return;
        }
    }
    if hidden_just_now(app) {
        return;
    }
    let app = app.clone();
    // Creating a WebView window from an event handler deadlocks on Windows
    // (WebView2), so do it on the async runtime.
    tauri::async_runtime::spawn(async move {
        let window = match app.get_webview_window(MENU_WINDOW) {
            Some(w) => w,
            None => match create_menu_window(&app) {
                Ok(w) => w,
                Err(e) => {
                    eprintln!("GitHop: couldn't open the tray menu: {e}");
                    crate::show_main_window(&app);
                    return;
                }
            },
        };
        let profiles = Store::open().map_or(0, |s| s.profiles().profiles.len());
        place_menu_window(&app, &window, icon, profiles);
        let _ = window.show();
        let _ = window.set_focus();
    });
}

fn create_menu_window<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<WebviewWindow<R>> {
    let window =
        WebviewWindowBuilder::new(app, MENU_WINDOW, WebviewUrl::App("index.html#tray".into()))
            .title("GitHop")
            .inner_size(MENU_WIDTH, menu_height(1))
            .decorations(false)
            .resizable(false)
            .maximizable(false)
            .minimizable(false)
            .skip_taskbar(true)
            .always_on_top(true)
            .shadow(true)
            .visible(false)
            .focused(true)
            .build()?;
    let handle = window.clone();
    window.on_window_event(move |event| {
        if let WindowEvent::Focused(false) = event {
            let _ = handle.hide();
            mark_menu_hidden(handle.app_handle());
        }
    });
    Ok(window)
}

/// Logical height for `profiles` rows (UI may resize itself to fit exactly).
fn menu_height(profiles: usize) -> f64 {
    (MENU_CHROME + MENU_ITEM * profiles.max(1) as f64).min(MENU_MAX_HEIGHT)
}

fn place_menu_window<R: Runtime>(
    app: &AppHandle<R>,
    window: &WebviewWindow<R>,
    icon: Rect,
    profiles: usize,
) {
    let probe = icon.position.to_physical::<f64>(1.0);
    let monitor = app
        .monitor_from_point(probe.x, probe.y)
        .ok()
        .flatten()
        .or_else(|| app.primary_monitor().ok().flatten());
    let Some(monitor) = monitor else {
        return;
    };
    let scale = monitor.scale_factor();
    let pos: PhysicalPosition<f64> = icon.position.to_physical(scale);
    let size: PhysicalSize<f64> = icon.size.to_physical(scale);
    let height = menu_height(profiles);
    let _ = window.set_size(tauri::LogicalSize::new(MENU_WIDTH, height));
    let work = monitor.work_area();
    let (x, y) = menu_position(
        Bounds {
            x: pos.x,
            y: pos.y,
            w: size.width,
            h: size.height,
        },
        (MENU_WIDTH * scale, height * scale),
        Bounds {
            x: f64::from(work.position.x),
            y: f64::from(work.position.y),
            w: f64::from(work.size.width),
            h: f64::from(work.size.height),
        },
        MENU_MARGIN * scale,
    );
    let _ = window.set_position(PhysicalPosition::new(x.round(), y.round()));
}

/// A rectangle in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Top-left for the menu window: centred on the icon, above it when the
/// taskbar is at the bottom, below when at the top, beside it when the taskbar
/// is vertical; always inside the work area.
pub fn menu_position(icon: Bounds, menu: (f64, f64), work: Bounds, margin: f64) -> (f64, f64) {
    let (mw, mh) = menu;
    let icon_cx = icon.x + icon.w / 2.0;
    let icon_cy = icon.y + icon.h / 2.0;
    let right = work.x + work.w;
    let bottom = work.y + work.h;

    let (x, y) = if icon.x >= right {
        // Taskbar on the right.
        (right - mw - margin, icon_cy - mh / 2.0)
    } else if icon.x + icon.w <= work.x {
        // Taskbar on the left.
        (work.x + margin, icon_cy - mh / 2.0)
    } else if icon_cy >= work.y + work.h / 2.0 {
        // Taskbar at the bottom (default).
        (icon_cx - mw / 2.0, icon.y.min(bottom) - mh - margin)
    } else {
        // Taskbar at the top.
        (icon_cx - mw / 2.0, (icon.y + icon.h).max(work.y) + margin)
    };
    let x = x.clamp(work.x + margin, (right - mw - margin).max(work.x + margin));
    let y = y.clamp(work.y + margin, (bottom - mh - margin).max(work.y + margin));
    (x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: Bounds = Bounds {
        x: 0.0,
        y: 0.0,
        w: 1920.0,
        h: 1040.0,
    };

    #[test]
    fn bottom_taskbar_opens_above_icon() {
        let icon = Bounds {
            x: 1700.0,
            y: 1045.0,
            w: 24.0,
            h: 30.0,
        };
        let (x, y) = menu_position(icon, (260.0, 200.0), WORK, 8.0);
        assert_eq!(x, 1712.0 - 130.0);
        assert_eq!(y, 1040.0 - 200.0 - 8.0);
    }

    #[test]
    fn stays_inside_right_edge() {
        let icon = Bounds {
            x: 1900.0,
            y: 1045.0,
            w: 20.0,
            h: 30.0,
        };
        let (x, _) = menu_position(icon, (260.0, 200.0), WORK, 8.0);
        assert_eq!(x, 1920.0 - 260.0 - 8.0);
    }

    #[test]
    fn top_taskbar_opens_below() {
        let work = Bounds {
            x: 0.0,
            y: 40.0,
            w: 1920.0,
            h: 1040.0,
        };
        let icon = Bounds {
            x: 1000.0,
            y: 5.0,
            w: 24.0,
            h: 30.0,
        };
        let (_, y) = menu_position(icon, (260.0, 200.0), work, 8.0);
        assert_eq!(y, 48.0);
    }

    #[test]
    fn side_taskbars() {
        let work = Bounds {
            x: 0.0,
            y: 0.0,
            w: 1860.0,
            h: 1080.0,
        };
        let icon = Bounds {
            x: 1870.0,
            y: 1000.0,
            w: 24.0,
            h: 24.0,
        };
        let (x, y) = menu_position(icon, (260.0, 200.0), work, 8.0);
        assert_eq!(x, 1860.0 - 260.0 - 8.0);
        assert!(y + 200.0 <= 1080.0 - 8.0);

        let work = Bounds {
            x: 60.0,
            y: 0.0,
            w: 1860.0,
            h: 1080.0,
        };
        let icon = Bounds {
            x: 10.0,
            y: 500.0,
            w: 24.0,
            h: 24.0,
        };
        let (x, y) = menu_position(icon, (260.0, 200.0), work, 8.0);
        assert_eq!(x, 68.0);
        assert_eq!(y, 512.0 - 100.0);
    }

    #[test]
    fn height_grows_with_profiles_and_is_capped() {
        assert_eq!(menu_height(0), menu_height(1));
        assert!(menu_height(3) > menu_height(2));
        assert_eq!(menu_height(100), MENU_MAX_HEIGHT);
    }

    #[test]
    fn menu_text_escapes_ampersands() {
        assert_eq!(menu_text("R&D", "a@b.c"), "R&&D\ta@b.c");
    }
}
