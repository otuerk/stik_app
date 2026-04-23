use crate::commands::{note_lock, notes, settings, sticked_notes};
use crate::state::{AppState, LastSavedNote};
use std::path::Path;
use sticked_notes::StickedNote;
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};

const COMMAND_PALETTE_WINDOW_WIDTH: f64 = 700.0;
const COMMAND_PALETTE_WINDOW_HEIGHT: f64 = 480.0;
const SETTINGS_WINDOW_WIDTH: f64 = 860.0;
const SETTINGS_WINDOW_HEIGHT: f64 = 720.0;
const SETTINGS_WINDOW_MIN_WIDTH: f64 = 760.0;
const SETTINGS_WINDOW_MIN_HEIGHT: f64 = 560.0;
const DEFAULT_CAPTURE_WINDOW_WIDTH: f64 = 400.0;
const DEFAULT_CAPTURE_WINDOW_HEIGHT: f64 = 280.0;
const DEFAULT_VIEWING_WINDOW_WIDTH: f64 = 450.0;
const DEFAULT_VIEWING_WINDOW_HEIGHT: f64 = 320.0;
const DEFAULT_STICKED_WINDOW_WIDTH: f64 = 400.0;
const DEFAULT_STICKED_WINDOW_HEIGHT: f64 = 280.0;
const APPLE_NOTES_PICKER_WINDOW_WIDTH: f64 = 550.0;
const APPLE_NOTES_PICKER_WINDOW_HEIGHT: f64 = 500.0;

/// Minimum overlap (in physical pixels) between window and monitor for the position to be usable.
const MIN_OVERLAP: f64 = 80.0;
const MAX_VIEWING_SLUG_LEN: usize = 48;

struct ViewingWindowIdentity {
    display_title: String,
    viewing_id: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct MonitorGeometry {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    scale_factor: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct WindowFrame {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl MonitorGeometry {
    fn from_monitor(monitor: &tauri::Monitor) -> Self {
        let work_area = monitor.work_area();
        Self {
            x: work_area.position.x as f64,
            y: work_area.position.y as f64,
            width: work_area.size.width as f64,
            height: work_area.size.height as f64,
            scale_factor: monitor.scale_factor().max(1.0),
        }
    }
}

fn available_monitor_geometries(app: &AppHandle) -> Vec<MonitorGeometry> {
    app.available_monitors()
        .unwrap_or_default()
        .iter()
        .map(MonitorGeometry::from_monitor)
        .collect()
}

fn logical_to_physical_size(logical_size: (f64, f64), scale_factor: f64) -> (f64, f64) {
    let scale_factor = scale_factor.max(1.0);
    (logical_size.0 * scale_factor, logical_size.1 * scale_factor)
}

fn overlap_dimensions(frame: WindowFrame, monitor: MonitorGeometry) -> (f64, f64) {
    let overlap_w = (frame.x + frame.width).min(monitor.x + monitor.width) - frame.x.max(monitor.x);
    let overlap_h =
        (frame.y + frame.height).min(monitor.y + monitor.height) - frame.y.max(monitor.y);

    (overlap_w.max(0.0), overlap_h.max(0.0))
}

fn clamp_origin(
    origin: (f64, f64),
    monitor: MonitorGeometry,
    physical_size: (f64, f64),
) -> (f64, f64) {
    let max_x = (monitor.x + monitor.width - physical_size.0).max(monitor.x);
    let max_y = (monitor.y + monitor.height - physical_size.1).max(monitor.y);

    (
        origin.0.clamp(monitor.x, max_x),
        origin.1.clamp(monitor.y, max_y),
    )
}

fn center_origin(monitor: MonitorGeometry, physical_size: (f64, f64)) -> (f64, f64) {
    let centered = (
        monitor.x + (monitor.width - physical_size.0) / 2.0,
        monitor.y + (monitor.height - physical_size.1) / 2.0,
    );
    clamp_origin(centered, monitor, physical_size)
}

fn find_source_monitor(
    monitors: &[MonitorGeometry],
    saved_origin: (f64, f64),
    logical_size: (f64, f64),
) -> Option<MonitorGeometry> {
    let mut best_match = None;
    let mut best_overlap_area = 0.0;

    for monitor in monitors {
        let physical_size = logical_to_physical_size(logical_size, monitor.scale_factor);
        let frame = WindowFrame {
            x: saved_origin.0,
            y: saved_origin.1,
            width: physical_size.0,
            height: physical_size.1,
        };
        let (overlap_w, overlap_h) = overlap_dimensions(frame, *monitor);
        let overlap_area = overlap_w * overlap_h;

        if overlap_w >= MIN_OVERLAP && overlap_h >= MIN_OVERLAP && overlap_area > best_overlap_area
        {
            best_overlap_area = overlap_area;
            best_match = Some(*monitor);
        }
    }

    best_match
}

fn resolve_window_origin(
    saved_origin: Option<(f64, f64)>,
    logical_size: (f64, f64),
    preserve_relative_offset: bool,
    target_monitor: MonitorGeometry,
    monitors: &[MonitorGeometry],
) -> (f64, f64) {
    let target_physical_size = logical_to_physical_size(logical_size, target_monitor.scale_factor);

    if preserve_relative_offset {
        if let Some(saved_origin) = saved_origin {
            if let Some(source_monitor) = find_source_monitor(monitors, saved_origin, logical_size)
            {
                let translated = (
                    target_monitor.x + (saved_origin.0 - source_monitor.x),
                    target_monitor.y + (saved_origin.1 - source_monitor.y),
                );
                return clamp_origin(translated, target_monitor, target_physical_size);
            }
        }
    }

    center_origin(target_monitor, target_physical_size)
}

fn current_window_logical_size(window: &WebviewWindow, fallback: (f64, f64)) -> (f64, f64) {
    let scale_factor = window.scale_factor().unwrap_or(1.0).max(1.0);
    window
        .outer_size()
        .map(|size| {
            (
                size.width as f64 / scale_factor,
                size.height as f64 / scale_factor,
            )
        })
        .unwrap_or(fallback)
}

fn primary_monitor_geometry(app: &AppHandle) -> Option<MonitorGeometry> {
    app.primary_monitor()
        .ok()
        .flatten()
        .map(|monitor| MonitorGeometry::from_monitor(&monitor))
}

fn monitor_from_screen_point(app: &AppHandle, x: f64, y: f64) -> Option<MonitorGeometry> {
    app.monitor_from_point(x, y)
        .ok()
        .flatten()
        .map(|monitor| MonitorGeometry::from_monitor(&monitor))
}

#[cfg(target_os = "macos")]
fn focused_window_frame() -> Option<WindowFrame> {
    use core_foundation::base::{CFTypeRef, TCFType};
    use core_foundation::string::{CFString, CFStringRef};
    use core_graphics::geometry::{CGPoint, CGSize};
    use std::ffi::c_void;

    unsafe extern "C" {
        fn AXUIElementCreateSystemWide() -> *mut c_void;
        fn AXUIElementCopyAttributeValue(
            element: *mut c_void,
            attribute: CFStringRef,
            value: *mut CFTypeRef,
        ) -> i32;
        fn AXValueGetValue(value: *const c_void, the_type: u32, value_ptr: *mut c_void) -> u8;
        fn CFRelease(cf: *const c_void);
    }

    const AX_ERROR_SUCCESS: i32 = 0;
    const AX_VALUE_TYPE_CG_POINT: u32 = 1;
    const AX_VALUE_TYPE_CG_SIZE: u32 = 2;

    unsafe {
        let systemwide = AXUIElementCreateSystemWide();
        if systemwide.is_null() {
            return None;
        }

        let focused_window_attr = CFString::from_static_string("AXFocusedWindow");
        let mut focused_window: CFTypeRef = std::ptr::null();
        let focused_status = AXUIElementCopyAttributeValue(
            systemwide,
            focused_window_attr.as_concrete_TypeRef(),
            &mut focused_window,
        );
        CFRelease(systemwide);

        if focused_status != AX_ERROR_SUCCESS || focused_window.is_null() {
            return None;
        }

        let position_attr = CFString::from_static_string("AXPosition");
        let mut position_value: CFTypeRef = std::ptr::null();
        let position_status = AXUIElementCopyAttributeValue(
            focused_window as *mut c_void,
            position_attr.as_concrete_TypeRef(),
            &mut position_value,
        );

        let size_attr = CFString::from_static_string("AXSize");
        let mut size_value: CFTypeRef = std::ptr::null();
        let size_status = AXUIElementCopyAttributeValue(
            focused_window as *mut c_void,
            size_attr.as_concrete_TypeRef(),
            &mut size_value,
        );

        CFRelease(focused_window);

        if position_status != AX_ERROR_SUCCESS
            || size_status != AX_ERROR_SUCCESS
            || position_value.is_null()
            || size_value.is_null()
        {
            if !position_value.is_null() {
                CFRelease(position_value);
            }
            if !size_value.is_null() {
                CFRelease(size_value);
            }
            return None;
        }

        let mut origin = CGPoint::new(0.0, 0.0);
        let mut size = CGSize::new(0.0, 0.0);
        let got_origin = AXValueGetValue(
            position_value,
            AX_VALUE_TYPE_CG_POINT,
            &mut origin as *mut _ as *mut c_void,
        ) != 0;
        let got_size = AXValueGetValue(
            size_value,
            AX_VALUE_TYPE_CG_SIZE,
            &mut size as *mut _ as *mut c_void,
        ) != 0;

        CFRelease(position_value);
        CFRelease(size_value);

        if !got_origin || !got_size {
            return None;
        }

        Some(WindowFrame {
            x: origin.x,
            y: origin.y,
            width: size.width,
            height: size.height,
        })
    }
}

#[cfg(target_os = "macos")]
fn cursor_position() -> Option<(f64, f64)> {
    use core_graphics::event::CGEvent;
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

    let source = CGEventSource::new(CGEventSourceStateID::CombinedSessionState).ok()?;
    let event = CGEvent::new(source).ok()?;
    let point = event.location();

    Some((point.x, point.y))
}

#[cfg(target_os = "macos")]
fn resolve_active_monitor(app: &AppHandle) -> Option<MonitorGeometry> {
    if let Some(frame) = focused_window_frame() {
        let focused_window_center = (frame.x + frame.width / 2.0, frame.y + frame.height / 2.0);
        if let Some(monitor) =
            monitor_from_screen_point(app, focused_window_center.0, focused_window_center.1)
        {
            return Some(monitor);
        }
    }

    if let Some((x, y)) = cursor_position() {
        if let Some(monitor) = monitor_from_screen_point(app, x, y) {
            return Some(monitor);
        }
    }

    primary_monitor_geometry(app)
}

#[cfg(not(target_os = "macos"))]
fn resolve_active_monitor(app: &AppHandle) -> Option<MonitorGeometry> {
    primary_monitor_geometry(app)
}

fn move_window_to_active_monitor(
    app: &AppHandle,
    window: &WebviewWindow,
    saved_origin: Option<(f64, f64)>,
    logical_size: (f64, f64),
    preserve_relative_offset: bool,
) {
    let Some(target_monitor) = resolve_active_monitor(app) else {
        return;
    };

    let monitors = available_monitor_geometries(app);
    let resolved_origin = resolve_window_origin(
        saved_origin,
        logical_size,
        preserve_relative_offset,
        target_monitor,
        &monitors,
    );

    let _ = window.set_position(tauri::Position::Physical(PhysicalPosition::new(
        resolved_origin.0.round() as i32,
        resolved_origin.1.round() as i32,
    )));
}

fn center_window_on_active_monitor(
    app: &AppHandle,
    window: &WebviewWindow,
    fallback_logical_size: (f64, f64),
) {
    let logical_size = current_window_logical_size(window, fallback_logical_size);
    move_window_to_active_monitor(app, window, None, logical_size, false);
}

fn place_capture_window(app: &AppHandle, window: &WebviewWindow) {
    let saved_settings = settings::load_settings_from_file().ok();
    if let Some((width, height)) = saved_settings.as_ref().and_then(|s| s.capture_window_size) {
        let _ = window.set_size(tauri::Size::Logical(tauri::LogicalSize::new(width, height)));
    }

    let logical_size = saved_settings
        .as_ref()
        .and_then(|s| s.capture_window_size)
        .unwrap_or_else(|| {
            current_window_logical_size(
                window,
                (DEFAULT_CAPTURE_WINDOW_WIDTH, DEFAULT_CAPTURE_WINDOW_HEIGHT),
            )
        });
    let saved_origin = saved_settings
        .as_ref()
        .and_then(|s| s.viewing_window_position);

    move_window_to_active_monitor(app, window, saved_origin, logical_size, true);
}

fn remember_last_note(state: &AppState, path: &str, folder: &str) {
    if path.trim().is_empty() {
        return;
    }

    let mut last = state
        .last_saved_note
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    *last = Some(LastSavedNote {
        path: path.to_string(),
        folder: folder.to_string(),
    });
}

fn is_break_placeholder_line(line: &str) -> bool {
    line.eq_ignore_ascii_case("<br>")
        || line.eq_ignore_ascii_case("<br/>")
        || line.eq_ignore_ascii_case("<br />")
}

fn normalize_display_title(line: &str) -> String {
    let trimmed = line.trim();
    let without_heading = trimmed.trim_start_matches('#').trim();
    without_heading
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn filename_title_from_path(path: &str) -> String {
    let stem = Path::new(path)
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("Untitled");

    let parts: Vec<&str> = stem.split('-').collect();
    let raw_title = if parts.len() > 3 {
        parts[2..parts.len() - 1].join(" ")
    } else {
        stem.replace('-', " ")
    };

    let title = raw_title.split_whitespace().collect::<Vec<_>>().join(" ");

    if title.is_empty() {
        "Untitled".to_string()
    } else {
        title.chars().take(120).collect()
    }
}

fn extract_display_title(content: &str, path: &str) -> String {
    if note_lock::is_locked_content(content) {
        return filename_title_from_path(path);
    }

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || is_break_placeholder_line(trimmed) {
            continue;
        }

        let normalized = normalize_display_title(trimmed);
        if !normalized.is_empty() {
            return normalized.chars().take(120).collect();
        }
    }

    filename_title_from_path(path)
}

fn sanitize_label_slug(title: &str) -> String {
    let mut slug = String::with_capacity(title.len().min(MAX_VIEWING_SLUG_LEN));
    let mut last_was_separator = false;

    for ch in title.chars() {
        if ch.is_ascii_alphanumeric() {
            if slug.len() >= MAX_VIEWING_SLUG_LEN {
                break;
            }
            slug.push(ch.to_ascii_lowercase());
            last_was_separator = false;
            continue;
        }

        if slug.is_empty() || last_was_separator {
            continue;
        }

        if slug.len() >= MAX_VIEWING_SLUG_LEN {
            break;
        }
        slug.push('-');
        last_was_separator = true;
    }

    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "untitled".to_string()
    } else {
        slug
    }
}

fn stable_path_hash(path: &str) -> String {
    const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    let mut hash = FNV_OFFSET_BASIS;
    for byte in path.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }

    let full = format!("{hash:016x}");
    full[..12].to_string()
}

fn derive_viewing_window_identity(content: &str, path: &str) -> ViewingWindowIdentity {
    let display_title = extract_display_title(content, path);
    let title_slug = sanitize_label_slug(&display_title);
    let path_hash = stable_path_hash(path);

    ViewingWindowIdentity {
        display_title,
        viewing_id: format!("view-{}-{}", title_slug, path_hash),
    }
}

pub fn show_postit_with_folder(app: &AppHandle, folder: &str) {
    if let Some(window) = app.get_webview_window("postit") {
        place_capture_window(app, &window);
        let _ = window.show();
        let _ = window.set_focus();
        let _ = window.emit("shortcut-triggered", folder);
    }
}

pub fn show_command_palette(app: &AppHandle) {
    {
        let state = app.state::<AppState>();
        let mut postit_visible = state
            .postit_was_visible
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *postit_visible = app
            .get_webview_window("postit")
            .map(|w| w.is_visible().unwrap_or(false))
            .unwrap_or(false);
    }

    for (label, window) in app.webview_windows() {
        if label.starts_with("sticked-") {
            let _ = window.set_always_on_top(false);
        }
    }

    if let Some(window) = app.get_webview_window("command-palette") {
        center_window_on_active_monitor(
            app,
            &window,
            (COMMAND_PALETTE_WINDOW_WIDTH, COMMAND_PALETTE_WINDOW_HEIGHT),
        );
        let _ = window.show();
        let _ = window.set_focus();
        return;
    }

    let window = WebviewWindowBuilder::new(
        app,
        "command-palette",
        WebviewUrl::App("index.html?window=command-palette".into()),
    )
    .title("Command Palette")
    .inner_size(COMMAND_PALETTE_WINDOW_WIDTH, COMMAND_PALETTE_WINDOW_HEIGHT)
    .resizable(false)
    .decorations(false)
    .transparent(true)
    .always_on_top(true)
    .skip_taskbar(true)
    .visible(false)
    .build();

    if let Ok(win) = window {
        let app_handle = app.clone();
        win.on_window_event(move |event| match event {
            tauri::WindowEvent::Focused(focused) => {
                if !focused {
                    for (label, window) in app_handle.webview_windows() {
                        if label.starts_with("sticked-") {
                            let _ = window.set_always_on_top(true);
                        }
                    }
                }
            }
            tauri::WindowEvent::Destroyed => {
                for (label, window) in app_handle.webview_windows() {
                    if label.starts_with("sticked-") {
                        let _ = window.set_always_on_top(true);
                    }
                }

                let state = app_handle.state::<AppState>();
                let postit_visible = *state
                    .postit_was_visible
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());

                if postit_visible {
                    let has_viewing_windows = app_handle
                        .webview_windows()
                        .iter()
                        .any(|(label, _)| label.starts_with("sticked-view-"));
                    if !has_viewing_windows {
                        if let Some(postit) = app_handle.get_webview_window("postit") {
                            let _ = postit.show();
                            let _ = postit.set_focus();
                        }
                    }
                }
            }
            _ => {}
        });

        center_window_on_active_monitor(
            app,
            &win,
            (COMMAND_PALETTE_WINDOW_WIDTH, COMMAND_PALETTE_WINDOW_HEIGHT),
        );
        let _ = win.show();
        let _ = win.set_focus();
    }
}

pub fn show_settings(app: &AppHandle) {
    {
        let state = app.state::<AppState>();
        let mut prev_window = state
            .previous_focused_window
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *prev_window = None;

        for (label, window) in app.webview_windows() {
            if label.starts_with("sticked-") {
                if window.is_focused().unwrap_or(false) {
                    *prev_window = Some(label.clone());
                    break;
                }
            }
        }

        let mut postit_visible = state
            .postit_was_visible
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *postit_visible = app
            .get_webview_window("postit")
            .map(|w| w.is_visible().unwrap_or(false))
            .unwrap_or(false);
    }

    for (label, window) in app.webview_windows() {
        if label.starts_with("sticked-") {
            let _ = window.set_always_on_top(false);
        }
    }

    if let Some(window) = app.get_webview_window("settings") {
        center_window_on_active_monitor(
            app,
            &window,
            (SETTINGS_WINDOW_WIDTH, SETTINGS_WINDOW_HEIGHT),
        );
        let _ = window.show();
        let _ = window.set_focus();
        return;
    }

    let window = WebviewWindowBuilder::new(
        app,
        "settings",
        WebviewUrl::App("index.html?window=settings".into()),
    )
    .title("Settings")
    .inner_size(SETTINGS_WINDOW_WIDTH, SETTINGS_WINDOW_HEIGHT)
    .min_inner_size(SETTINGS_WINDOW_MIN_WIDTH, SETTINGS_WINDOW_MIN_HEIGHT)
    .resizable(true)
    .decorations(false)
    .transparent(true)
    .always_on_top(true)
    .skip_taskbar(true)
    .visible(false)
    .build();

    if let Ok(win) = window {
        let app_handle = app.clone();
        win.on_window_event(move |event| {
            if let tauri::WindowEvent::Destroyed = event {
                for (label, window) in app_handle.webview_windows() {
                    if label.starts_with("sticked-") {
                        let _ = window.set_always_on_top(true);
                    }
                }

                let state = app_handle.state::<AppState>();
                let prev_window = state
                    .previous_focused_window
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let postit_visible = *state
                    .postit_was_visible
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());

                if let Some(label) = prev_window.as_ref() {
                    if let Some(window) = app_handle.get_webview_window(label) {
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                } else if postit_visible {
                    if let Some(postit) = app_handle.get_webview_window("postit") {
                        let _ = postit.show();
                        let _ = postit.set_focus();
                    }
                }
            }
        });

        center_window_on_active_monitor(app, &win, (SETTINGS_WINDOW_WIDTH, SETTINGS_WINDOW_HEIGHT));
        let _ = win.show();
        let _ = win.set_focus();
    }
}

#[tauri::command]
pub fn hide_window(window: tauri::Window) {
    let _ = window.hide();
}

#[tauri::command]
pub fn hide_postit(app: AppHandle) {
    if let Some(window) = app.get_webview_window("postit") {
        let _ = window.hide();
    }
}

#[tauri::command]
pub fn create_sticked_window(app: AppHandle, note: StickedNote) -> Result<bool, String> {
    let window_label = format!("sticked-{}", note.id);

    if app.get_webview_window(&window_label).is_some() {
        return Ok(true);
    }

    let saved_position = note.position;
    let (width, height) = note.size.unwrap_or((400.0, 280.0));
    let url = format!("index.html?window=sticked&id={}", note.id);

    // Build hidden — position after creation using PhysicalPosition to avoid
    // the logical/physical mismatch in WebviewWindowBuilder::position().
    let window = WebviewWindowBuilder::new(&app, &window_label, WebviewUrl::App(url.into()))
        .title("Sticked Note")
        .inner_size(width, height)
        .min_inner_size(320.0, 200.0)
        .max_inner_size(800.0, 600.0)
        .resizable(true)
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible(false)
        .build();

    match window {
        Ok(win) => {
            if let Some((x, y)) = saved_position {
                let _ = win.set_position(tauri::Position::Physical(PhysicalPosition::new(
                    x as i32, y as i32,
                )));
            } else {
                let _ = win.center();
            }
            let _ = win.show();
            Ok(true)
        }
        Err(e) => Err(format!("Failed to create sticked window: {}", e)),
    }
}

pub fn create_sticked_window_centered(app: AppHandle, note: StickedNote) -> Result<bool, String> {
    let window_label = format!("sticked-{}", note.id);

    if app.get_webview_window(&window_label).is_some() {
        return Ok(true);
    }

    let (width, height) = note.size.unwrap_or((400.0, 280.0));
    let url = format!("index.html?window=sticked&id={}", note.id);

    let window = WebviewWindowBuilder::new(&app, &window_label, WebviewUrl::App(url.into()))
        .title("Sticked Note")
        .inner_size(width, height)
        .min_inner_size(320.0, 200.0)
        .max_inner_size(800.0, 600.0)
        .resizable(true)
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible(false)
        .build();

    match window {
        Ok(win) => {
            center_window_on_active_monitor(&app, &win, (width, height));
            let _ = win.show();
            Ok(true)
        }
        Err(e) => Err(format!("Failed to create sticked window: {}", e)),
    }
}

#[tauri::command]
pub fn close_sticked_window(app: AppHandle, id: String) -> Result<bool, String> {
    let window_label = format!("sticked-{}", id);

    if let Some(window) = app.get_webview_window(&window_label) {
        let _ = window.close();
    }

    // Clean up viewing note cache to prevent memory leak
    if id.starts_with("view-") {
        let state = app.state::<AppState>();
        let mut viewing_notes = state
            .viewing_notes
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        viewing_notes.remove(&id);
    }

    Ok(true)
}

#[tauri::command]
pub async fn pin_capture_note(
    app: AppHandle,
    content: String,
    folder: String,
) -> Result<StickedNote, String> {
    // Read saved viewing position so the pinned note opens where the last
    // sticked/viewing window was, not always centered.
    let saved = settings::load_settings_from_file().ok();
    let saved_pos = saved.as_ref().and_then(|s| s.viewing_window_position);
    let logical_size = saved
        .as_ref()
        .and_then(|s| s.viewing_window_size)
        .unwrap_or((DEFAULT_STICKED_WINDOW_WIDTH, DEFAULT_STICKED_WINDOW_HEIGHT));

    let mut note = sticked_notes::create_sticked_note(content, folder, None)?;
    let monitors = available_monitor_geometries(&app);

    if let Some(target_monitor) = resolve_active_monitor(&app) {
        let resolved_origin =
            resolve_window_origin(saved_pos, logical_size, true, target_monitor, &monitors);
        note.position = Some(resolved_origin);
    }

    if saved.as_ref().and_then(|s| s.viewing_window_size).is_some() {
        note.size = Some(logical_size);
    }

    if note.position.is_some() {
        create_sticked_window(app.clone(), note.clone())?;
    } else {
        create_sticked_window_centered(app.clone(), note.clone())?;
    }

    // Persist the actual window position/size so it restores correctly
    let window_label = format!("sticked-{}", note.id);
    if let Some(win) = app.get_webview_window(&window_label) {
        if let (Ok(pos), Ok(size)) = (win.outer_position(), win.outer_size()) {
            let _ = sticked_notes::update_sticked_note(
                note.id.clone(),
                None,
                None,
                Some((pos.x as f64, pos.y as f64)),
                Some((size.width as f64, size.height as f64)),
            );

            // Keep the global viewing geometry in sync.
            let scale = win.scale_factor().unwrap_or(1.0);
            let lw = size.width as f64 / scale;
            let lh = size.height as f64 / scale;
            let _ = settings::save_viewing_window_geometry(lw, lh, pos.x as f64, pos.y as f64);
        }
    }

    if let Some(window) = app.get_webview_window("postit") {
        let _ = window.hide();
    }

    Ok(note)
}

#[tauri::command]
pub async fn open_note_for_viewing(
    app: AppHandle,
    content: String,
    folder: String,
    path: String,
) -> Result<bool, String> {
    {
        let state = app.state::<AppState>();
        remember_last_note(&state, &path, &folder);
    }

    let identity = derive_viewing_window_identity(&content, &path);
    let window_label = format!("sticked-{}", identity.viewing_id);

    if app.get_webview_window(&window_label).is_some() {
        return Ok(true);
    }

    {
        let state = app.state::<AppState>();
        let mut viewing_notes = state
            .viewing_notes
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        viewing_notes.insert(
            identity.viewing_id.clone(),
            crate::state::ViewingNoteContent {
                id: identity.viewing_id.clone(),
                content,
                folder,
                path: path.clone(),
            },
        );
    }

    let url = format!(
        "index.html?window=sticked&id={}&viewing=true",
        identity.viewing_id
    );

    let saved_settings = settings::load_settings_from_file().ok();
    let (width, height) = saved_settings
        .as_ref()
        .and_then(|s| s.viewing_window_size)
        .unwrap_or((DEFAULT_VIEWING_WINDOW_WIDTH, DEFAULT_VIEWING_WINDOW_HEIGHT));
    let saved_position = saved_settings
        .as_ref()
        .and_then(|s| s.viewing_window_position);

    // Build hidden — we position after creation using PhysicalPosition to avoid
    // the logical/physical mismatch in WebviewWindowBuilder::position().
    let builder = WebviewWindowBuilder::new(&app, &window_label, WebviewUrl::App(url.into()))
        .title(identity.display_title.clone())
        .inner_size(width, height)
        .min_inner_size(320.0, 200.0)
        .max_inner_size(800.0, 600.0)
        .resizable(true)
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible(false);

    let window = builder.build();

    match window {
        Ok(win) => {
            move_window_to_active_monitor(&app, &win, saved_position, (width, height), true);
            let _ = win.show();
            let _ = win.set_focus();
            Ok(true)
        }
        Err(e) => Err(format!("Failed to create viewing window: {}", e)),
    }
}

#[tauri::command]
pub fn get_viewing_note_content(app: AppHandle, id: String) -> Result<serde_json::Value, String> {
    let state = app.state::<AppState>();
    let viewing_notes = state
        .viewing_notes
        .lock()
        .unwrap_or_else(|e| e.into_inner());

    if let Some(note) = viewing_notes.get(&id) {
        Ok(serde_json::json!({
            "id": note.id,
            "content": note.content,
            "folder": note.folder,
            "path": note.path
        }))
    } else {
        Err("Viewing note content not found".to_string())
    }
}

#[tauri::command]
pub fn transfer_to_capture(
    app: AppHandle,
    content: String,
    folder: String,
) -> Result<bool, String> {
    if let Some(window) = app.get_webview_window("postit") {
        place_capture_window(&app, &window);
        let _ = window.show();
        let _ = window.set_focus();
        let _ = window.emit(
            "transfer-content",
            serde_json::json!({
                "content": content,
                "folder": folder
            }),
        );
        Ok(true)
    } else {
        Err("Postit window not found".to_string())
    }
}

#[tauri::command]
pub fn open_command_palette(app: AppHandle) -> Result<bool, String> {
    show_command_palette(&app);
    Ok(true)
}

#[tauri::command]
pub fn open_search(app: AppHandle) -> Result<bool, String> {
    show_command_palette(&app);
    Ok(true)
}

#[tauri::command]
pub fn open_manager(app: AppHandle) -> Result<bool, String> {
    show_command_palette(&app);
    Ok(true)
}

#[tauri::command]
pub fn open_settings(app: AppHandle) -> Result<bool, String> {
    show_settings(&app);
    Ok(true)
}

#[tauri::command]
pub async fn reopen_last_note(app: AppHandle) -> Result<bool, String> {
    let (path, folder) = {
        let state = app.state::<AppState>();
        let last = state
            .last_saved_note
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        match last.as_ref() {
            Some(note) => (note.path.clone(), note.folder.clone()),
            None => return Err("No note saved yet".to_string()),
        }
    };

    let content = notes::get_note_content_inner(&path)?;
    open_note_for_viewing(app, content, folder, path).await
}

pub fn show_apple_notes_picker(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("apple-notes-picker") {
        center_window_on_active_monitor(
            app,
            &window,
            (
                APPLE_NOTES_PICKER_WINDOW_WIDTH,
                APPLE_NOTES_PICKER_WINDOW_HEIGHT,
            ),
        );
        let _ = window.show();
        let _ = window.set_focus();
        return;
    }

    let window = WebviewWindowBuilder::new(
        app,
        "apple-notes-picker",
        WebviewUrl::App("index.html?window=apple-notes-picker".into()),
    )
    .title("Import from Apple Notes")
    .inner_size(
        APPLE_NOTES_PICKER_WINDOW_WIDTH,
        APPLE_NOTES_PICKER_WINDOW_HEIGHT,
    )
    .resizable(false)
    .decorations(false)
    .transparent(true)
    .always_on_top(true)
    .skip_taskbar(true)
    .visible(false)
    .build();

    if let Ok(win) = window {
        let app_handle = app.clone();
        win.on_window_event(move |event| {
            if let tauri::WindowEvent::Focused(focused) = event {
                if !focused {
                    if let Some(w) = app_handle.get_webview_window("apple-notes-picker") {
                        let _ = w.close();
                    }
                }
            }
        });

        center_window_on_active_monitor(
            app,
            &win,
            (
                APPLE_NOTES_PICKER_WINDOW_WIDTH,
                APPLE_NOTES_PICKER_WINDOW_HEIGHT,
            ),
        );
        let _ = win.show();
        let _ = win.set_focus();
    }
}

#[tauri::command]
pub fn show_apple_notes_picker_cmd(app: AppHandle) -> Result<bool, String> {
    show_apple_notes_picker(&app);
    Ok(true)
}

pub fn restore_sticked_notes(app: &AppHandle) {
    if let Ok(notes) = sticked_notes::list_sticked_notes() {
        for note in notes {
            let _ = create_sticked_window(app.clone(), note);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        derive_viewing_window_identity, remember_last_note, resolve_window_origin, MonitorGeometry,
        SETTINGS_WINDOW_MIN_WIDTH, SETTINGS_WINDOW_WIDTH,
    };
    use crate::state::AppState;

    #[test]
    fn remember_last_note_updates_state_for_shortcuts() {
        let state = AppState::new();
        remember_last_note(&state, "/tmp/stik/foo.md", "Inbox");

        let last = state
            .last_saved_note
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let note = last.as_ref().expect("last note should be set");
        assert_eq!(note.path, "/tmp/stik/foo.md");
        assert_eq!(note.folder, "Inbox");
    }

    #[test]
    fn settings_window_min_width_is_large_enough_for_full_menu_bar() {
        assert!(SETTINGS_WINDOW_MIN_WIDTH >= 760.0);
        assert!(SETTINGS_WINDOW_WIDTH > SETTINGS_WINDOW_MIN_WIDTH);
    }

    #[test]
    fn preserve_relative_offset_on_same_monitor_keeps_saved_origin() {
        let monitors = vec![MonitorGeometry {
            x: 0.0,
            y: 0.0,
            width: 1600.0,
            height: 900.0,
            scale_factor: 2.0,
        }];

        let origin = resolve_window_origin(
            Some((120.0, 150.0)),
            (400.0, 300.0),
            true,
            monitors[0],
            &monitors,
        );

        assert_eq!(origin, (120.0, 150.0));
    }

    #[test]
    fn preserve_relative_offset_moves_window_to_active_monitor() {
        let monitors = vec![
            MonitorGeometry {
                x: 0.0,
                y: 0.0,
                width: 1600.0,
                height: 900.0,
                scale_factor: 2.0,
            },
            MonitorGeometry {
                x: 1600.0,
                y: 0.0,
                width: 1920.0,
                height: 1080.0,
                scale_factor: 1.0,
            },
        ];

        let origin = resolve_window_origin(
            Some((200.0, 100.0)),
            (400.0, 300.0),
            true,
            monitors[1],
            &monitors,
        );

        assert_eq!(origin, (1800.0, 100.0));
    }

    #[test]
    fn mixed_scale_translation_clamps_using_target_monitor_scale() {
        let monitors = vec![
            MonitorGeometry {
                x: 0.0,
                y: 0.0,
                width: 1512.0,
                height: 982.0,
                scale_factor: 2.0,
            },
            MonitorGeometry {
                x: 1512.0,
                y: 0.0,
                width: 1000.0,
                height: 700.0,
                scale_factor: 1.0,
            },
        ];

        let origin = resolve_window_origin(
            Some((1200.0, 500.0)),
            (400.0, 300.0),
            true,
            monitors[1],
            &monitors,
        );

        assert_eq!(origin, (2112.0, 400.0));
    }

    #[test]
    fn invalid_saved_origin_falls_back_to_center_on_target_monitor() {
        let monitors = vec![MonitorGeometry {
            x: 0.0,
            y: 0.0,
            width: 1920.0,
            height: 1080.0,
            scale_factor: 1.0,
        }];

        let origin = resolve_window_origin(
            Some((5000.0, 50.0)),
            (400.0, 300.0),
            true,
            monitors[0],
            &monitors,
        );

        assert_eq!(origin, (760.0, 390.0));
    }

    #[test]
    fn viewing_identity_keeps_icloud_paths_tauri_safe() {
        let identity = derive_viewing_window_identity(
            "# This is my last note\n\nBody",
            "/Users/test/Library/Mobile Documents/com~apple~CloudDocs/Stik/Inbox/20260423-084615-this-is-my-last-note-a1b2.md",
        );

        assert_eq!(identity.display_title, "This is my last note");
        assert!(
            identity
                .viewing_id
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_'),
            "viewing id should contain only Tauri-safe characters: {}",
            identity.viewing_id
        );
        assert!(!identity.viewing_id.contains('~'));
        assert!(identity
            .viewing_id
            .starts_with("view-this-is-my-last-note-"));
    }

    #[test]
    fn viewing_identity_uses_hash_to_avoid_duplicate_title_collisions() {
        let first = derive_viewing_window_identity(
            "# Same Title",
            "/Users/test/Documents/Stik/Inbox/20260423-084615-same-title-a1b2.md",
        );
        let second = derive_viewing_window_identity(
            "# Same Title",
            "/Users/test/Documents/Stik/Work/20260423-084700-same-title-c3d4.md",
        );

        assert_eq!(first.display_title, "Same Title");
        assert_eq!(second.display_title, "Same Title");
        assert_ne!(first.viewing_id, second.viewing_id);
    }

    #[test]
    fn viewing_identity_is_stable_for_the_same_path() {
        let path = "/Users/test/Documents/Stik/Inbox/20260423-084615-repeatable-a1b2.md";
        let first = derive_viewing_window_identity("# Repeatable", path);
        let second = derive_viewing_window_identity("# Repeatable", path);

        assert_eq!(first.viewing_id, second.viewing_id);
    }

    #[test]
    fn viewing_identity_keeps_unicode_title_readable_but_slug_safe() {
        let identity = derive_viewing_window_identity(
            "## Café (Project) / 東京\n\nBody",
            "/Users/test/Documents/Stik/Inbox/20260423-084615-cafe-project-a1b2.md",
        );

        assert_eq!(identity.display_title, "Café (Project) / 東京");
        assert!(identity.viewing_id.starts_with("view-caf-project-"));
    }

    #[test]
    fn viewing_identity_falls_back_to_filename_for_locked_notes() {
        let identity = derive_viewing_window_identity(
            "---stik-locked---\nnonce: abc\ndata",
            "/Users/test/Documents/Stik/Inbox/20260423-084615-my-secret-note-a1b2.md",
        );

        assert_eq!(identity.display_title, "my secret note");
        assert!(identity.viewing_id.starts_with("view-my-secret-note-"));
    }
}
