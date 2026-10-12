use crate::config::ConfigState;
use crate::persistence::StateStore;
use crate::state::SharedState;
use crate::tab_controller::{NOTION_HOME, TAB_STRIP_HEIGHT};
use crate::traits::{SecuritySandbox, TabOrchestrator};
use std::sync::Arc;
use tauri::{
    AppHandle, LogicalPosition, LogicalSize, Manager, Runtime, Webview, WebviewWindowBuilder, Window,
};

pub const MAIN_WINDOW: &str = "main";

// Notion desktop: default 1320x860, minimum 640x480.
const MIN_WIDTH: f64 = 640.0;
const MIN_HEIGHT: f64 = 480.0;
const DEFAULT_WIDTH: f64 = 1320.0;
const DEFAULT_HEIGHT: f64 = 860.0;

/// Logical position/size of the Notion content area (everything below the
/// tab strip).
pub fn content_bounds<R: Runtime>(
    window: &Window<R>,
) -> tauri::Result<(LogicalPosition<f64>, LogicalSize<f64>)> {
    let scale = window.scale_factor()?;
    let size = window.inner_size()?.to_logical::<f64>(scale);
    Ok((
        LogicalPosition::new(0.0, TAB_STRIP_HEIGHT),
        LogicalSize::new(size.width, (size.height - TAB_STRIP_HEIGHT).max(0.0)),
    ))
}

fn orchestrator<R: Runtime>(app: &AppHandle<R>) -> Option<Arc<dyn TabOrchestrator<R>>> {
    app.try_state::<Arc<dyn TabOrchestrator<R>>>()
        .map(|o| o.inner().clone())
}

fn strip<R: Runtime>(app: &AppHandle<R>) -> Option<Webview<R>> {
    // The strip shares its native window with the Notion child webviews.
    // get_webview_window only succeeds for windows with a single webview.
    app.get_webview(MAIN_WINDOW)
}

fn persist<R: Runtime>(app: &AppHandle<R>, update: impl FnOnce(&mut crate::state::AppState)) {
    let (Some(state), Some(store)) = (
        app.try_state::<SharedState>(),
        app.try_state::<Arc<StateStore>>(),
    ) else {
        return;
    };
    let mut state = state.lock().expect("AppState lock poisoned");
    update(&mut state);
    store.save(&state);
}

/// Push the current tab list to the tab strip and persist it.
pub fn tabs_changed<R: Runtime>(app: &AppHandle<R>) {
    let Some(orchestrator) = orchestrator(app) else {
        return;
    };
    let tabs = orchestrator.tab_states();
    log::debug!(
        "tabs changed: {:?}",
        tabs.iter().map(|t| (t.title.as_str(), t.is_active)).collect::<Vec<_>>()
    );

    if let Some(strip) = strip(app) {
        if let Ok(json) = serde_json::to_string(&tabs) {
            let _ = strip.eval(format!("window.__lotionRender && window.__lotionRender({json});"));
        }
    }
    if let Some(window) = app.get_window(MAIN_WINDOW) {
        if let Some(active) = tabs.iter().find(|t| t.is_active) {
            let title = if active.title.is_empty() { "Notion" } else { &active.title };
            let _ = window.set_title(title);
        }
    }

    persist(app, |state| {
        let window = state.windows.entry(MAIN_WINDOW.to_string()).or_insert_with(|| {
            crate::state::WindowState::new(MAIN_WINDOW)
        });
        window.tab_ids = tabs.iter().map(|t| t.id.clone()).collect();
        window.active_tab_id = tabs.iter().find(|t| t.is_active).map(|t| t.id.clone());
        state.tabs = tabs.into_iter().map(|t| (t.id.clone(), t)).collect();
    });
}

pub fn tab_title_changed<R: Runtime>(app: &AppHandle<R>, webview: &tauri::Webview<R>, title: String) {
    #[cfg(debug_assertions)]
    if let Some(report) = title.strip_prefix(crate::diagnostics::TITLE_PREFIX) {
        log::info!("[diag] tab {}: {}", webview.label(), report);
        return;
    }
    // Suspended tabs sit on an untitled about:blank; keep their real title.
    if title.trim().is_empty() {
        return;
    }
    let Some(orchestrator) = orchestrator(app) else {
        return;
    };
    // Notion updates the URL with pushState, which fires no navigation event;
    // a title change is the reliable signal that the page changed.
    let url = webview
        .url()
        .ok()
        .map(|u| u.to_string())
        .filter(|u| u != "about:blank");
    if orchestrator.update_tab_meta(webview.label(), Some(title), url) {
        tabs_changed(app);
    }
}

pub fn tab_url_changed<R: Runtime>(app: &AppHandle<R>, tab_id: &str, url: &str) {
    if let Some(orchestrator) = orchestrator(app) {
        if orchestrator.update_tab_meta(tab_id, None, Some(url.to_string())) {
            tabs_changed(app);
        }
    }
}

/// Show a short message in the tab strip (e.g. finished downloads).
pub fn toast<R: Runtime>(app: &AppHandle<R>, message: &str) {
    if let (Some(strip), Ok(json)) = (strip(app), serde_json::to_string(message)) {
        let _ = strip.eval(format!("window.__lotionToast && window.__lotionToast({json});"));
    }
}

// ── User-facing tab actions (menu, shortcuts, tab strip) ────────────────────

pub fn new_tab<R: Runtime>(app: &AppHandle<R>, url: Option<&str>) {
    let Some(orchestrator) = orchestrator(app) else {
        return;
    };
    match orchestrator.create_tab(app, MAIN_WINDOW, url.unwrap_or(NOTION_HOME)) {
        Ok(id) => {
            apply_zoom_to(app, &id);
            let _ = orchestrator.show_tab(&id);
            tabs_changed(app);
        }
        Err(e) => log::error!("Failed to create tab: {}", e),
    }
}

pub fn close_tab<R: Runtime>(app: &AppHandle<R>, tab_id: &str) {
    let Some(orchestrator) = orchestrator(app) else {
        return;
    };
    if orchestrator.get_tab_ids().len() <= 1 {
        // Closing the last tab closes the window (and on macOS keeps the app
        // running, like Notion). A fresh tab is ready when it comes back.
        let _ = orchestrator.destroy_tab(tab_id);
        new_tab(app, None);
        close_main_window(app);
        return;
    }
    let _ = orchestrator.destroy_tab(tab_id);
    tabs_changed(app);
}

pub fn close_active_tab<R: Runtime>(app: &AppHandle<R>) {
    if let Some(id) = orchestrator(app).and_then(|o| o.active_tab_id()) {
        close_tab(app, &id);
    }
}

pub fn reopen_closed_tab<R: Runtime>(app: &AppHandle<R>) {
    if let Some(url) = orchestrator(app).and_then(|o| o.pop_closed_tab()) {
        new_tab(app, Some(&url));
    }
}

pub fn switch_tab<R: Runtime>(app: &AppHandle<R>, tab_id: &str) {
    if let Some(orchestrator) = orchestrator(app) {
        let _ = orchestrator.show_tab(tab_id);
        tabs_changed(app);
    }
}

/// Select the tab `offset` positions away from the active one (wrapping).
pub fn select_relative_tab<R: Runtime>(app: &AppHandle<R>, offset: isize) {
    let Some(orchestrator) = orchestrator(app) else {
        return;
    };
    let ids = orchestrator.get_tab_ids();
    if ids.is_empty() {
        return;
    }
    let current = orchestrator
        .active_tab_id()
        .and_then(|a| ids.iter().position(|id| *id == a))
        .unwrap_or(0) as isize;
    let next = (current + offset).rem_euclid(ids.len() as isize) as usize;
    switch_tab(app, &ids[next]);
}

/// Cmd+1..8 select by position, Cmd+9 selects the last tab (browser convention).
pub fn select_tab_number<R: Runtime>(app: &AppHandle<R>, number: usize) {
    let Some(orchestrator) = orchestrator(app) else {
        return;
    };
    let ids = orchestrator.get_tab_ids();
    let target = if number >= 9 { ids.last() } else { ids.get(number.saturating_sub(1)) };
    if let Some(id) = target.cloned() {
        switch_tab(app, &id);
    }
}

pub fn move_tab<R: Runtime>(app: &AppHandle<R>, tab_id: &str, to_index: usize) {
    if let Some(orchestrator) = orchestrator(app) {
        orchestrator.move_tab(tab_id, to_index);
        tabs_changed(app);
    }
}

fn eval_active<R: Runtime>(app: &AppHandle<R>, js: &str) {
    if let Some(webview) = orchestrator(app).and_then(|o| o.active_webview()) {
        let _ = webview.eval(js);
    }
}

pub fn go_back<R: Runtime>(app: &AppHandle<R>) {
    eval_active(app, "history.back();");
}

pub fn go_forward<R: Runtime>(app: &AppHandle<R>) {
    eval_active(app, "history.forward();");
}

pub fn reload_active<R: Runtime>(app: &AppHandle<R>) {
    if let Some(webview) = orchestrator(app).and_then(|o| o.active_webview()) {
        let _ = webview.reload();
    }
}

/// Reload bypassing the HTTP cache (WebKit's reloadFromOrigin).
pub fn force_reload_active<R: Runtime>(app: &AppHandle<R>) {
    let Some(webview) = orchestrator(app).and_then(|o| o.active_webview()) else {
        return;
    };
    #[cfg(target_os = "macos")]
    {
        let _ = webview.with_webview(|platform| unsafe {
            let wk: &objc2_web_kit::WKWebView = &*platform.inner().cast();
            wk.reloadFromOrigin();
        });
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = webview.reload();
    }
}

pub fn go_home<R: Runtime>(app: &AppHandle<R>) {
    if let Some(webview) = orchestrator(app).and_then(|o| o.active_webview()) {
        let _ = webview.navigate(NOTION_HOME.parse().expect("valid url"));
    }
}

#[cfg(debug_assertions)]
pub fn open_devtools<R: Runtime>(app: &AppHandle<R>) {
    if let Some(webview) = orchestrator(app).and_then(|o| o.active_webview()) {
        webview.open_devtools();
    }
}

#[cfg(not(debug_assertions))]
pub fn open_devtools<R: Runtime>(_app: &AppHandle<R>) {
    log::info!("Developer tools are disabled in release builds");
}

/// Notion desktop's zoom range: 0.5–4.9 in 0.1 steps.
const ZOOM_MIN: f64 = 0.5;
const ZOOM_MAX: f64 = 4.9;
const ZOOM_STEP: f64 = 0.1;

/// `direction`: +1 zoom in, -1 zoom out, 0 reset. Applies to every tab and
/// persists in the config, like Notion's app-wide zoom.
pub fn zoom<R: Runtime>(app: &AppHandle<R>, direction: i32) {
    let Some(config) = app.try_state::<ConfigState>() else {
        return;
    };
    let level = {
        let mut config = config.lock().expect("config lock poisoned");
        let current = config.zoom;
        let level = match direction {
            0 => 1.0,
            d => {
                let next = current + ZOOM_STEP * d.signum() as f64;
                ((next * 10.0).round() / 10.0).clamp(ZOOM_MIN, ZOOM_MAX)
            }
        };
        config.zoom = level;
        let _ = config.save();
        level
    };
    if let Some(orchestrator) = orchestrator(app) {
        for webview in orchestrator.get_tab_ids().iter().filter_map(|id| app.get_webview(id)) {
            let _ = webview.set_zoom(level);
        }
    }
}

fn apply_zoom_to<R: Runtime>(app: &AppHandle<R>, tab_id: &str) {
    let level = app
        .try_state::<ConfigState>()
        .map(|c| c.lock().expect("config lock poisoned").zoom)
        .unwrap_or(1.0);
    if (level - 1.0).abs() > f64::EPSILON {
        if let Some(webview) = app.get_webview(tab_id) {
            let _ = webview.set_zoom(level);
        }
    }
}

pub fn show_main_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_window(MAIN_WINDOW) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
        if let Some(webview) = orchestrator(app).and_then(|o| o.active_webview()) {
            let _ = webview.set_focus();
        }
    }
}

/// macOS: hide instead of destroying (the app stays in the Dock and the window
/// comes back instantly with all tabs). Elsewhere: quit.
pub fn close_main_window<R: Runtime>(app: &AppHandle<R>) {
    #[cfg(target_os = "macos")]
    {
        if let Some(window) = app.get_window(MAIN_WINDOW) {
            if window.is_fullscreen().unwrap_or(false) {
                // Hiding a fullscreen window leaves an empty Space behind.
                let _ = window.set_fullscreen(false);
            }
            let _ = window.hide();
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        app.exit(0);
    }
}

pub struct WindowController<R: Runtime> {
    pub window: Window<R>,
    pub security: Arc<dyn SecuritySandbox>,
}

impl<R: Runtime> WindowController<R> {
    pub fn new(app: &AppHandle<R>, security: Arc<dyn SecuritySandbox>) -> tauri::Result<Self> {
        let saved_bounds = app.try_state::<SharedState>().and_then(|s| {
            let state = s.lock().expect("AppState lock poisoned");
            state.windows.get(MAIN_WINDOW).map(|w| (w.bounds.clone(), w.is_maximized))
        });

        let (width, height) = saved_bounds
            .as_ref()
            .map(|(b, _)| (b.width.max(MIN_WIDTH), b.height.max(MIN_HEIGHT)))
            .unwrap_or((DEFAULT_WIDTH, DEFAULT_HEIGHT));

        let platform = if cfg!(target_os = "macos") {
            "macos"
        } else if cfg!(target_os = "windows") {
            "windows"
        } else {
            "linux"
        };

        // Window chrome lives in tauri.conf.json (`create: false`): on macOS
        // that is the only way to reach tao's traffic-light inset, which keeps
        // the native buttons centred in our 36px tab strip (like Notion's
        // `titleBarStyle: hiddenInset`) and survives resizes/fullscreen.
        let window_config = app
            .config()
            .app
            .windows
            .iter()
            .find(|w| w.label == MAIN_WINDOW)
            .cloned()
            .ok_or_else(|| tauri::Error::AssetNotFound("main window config".into()))?;

        let builder = WebviewWindowBuilder::from_config(app, &window_config)?
            .inner_size(width, height)
            .min_inner_size(MIN_WIDTH, MIN_HEIGHT)
            .initialization_script(format!("window.__LOTION_PLATFORM__ = {:?};", platform));

        let main = builder.build()?;
        let window = main.as_ref().window();

        if let Some((bounds, maximized)) = saved_bounds {
            if let (Some(x), Some(y)) = (bounds.x, bounds.y) {
                if position_is_visible(&window, x, y, width) {
                    let _ = window.set_position(LogicalPosition::new(x, y));
                } else {
                    let _ = window.center();
                }
            } else {
                let _ = window.center();
            }
            if maximized {
                let _ = window.maximize();
            }
        } else {
            let _ = window.center();
        }

        Ok(Self { window, security })
    }

    pub fn setup_listeners(&self, app_handle: AppHandle<R>) {
        let window_label = self.window.label().to_string();
        let window = self.window.clone();

        self.window.on_window_event(move |event| match event {
            tauri::WindowEvent::CloseRequested { api, .. } => {
                #[cfg(target_os = "macos")]
                {
                    api.prevent_close();
                    close_main_window(&app_handle);
                }
                #[cfg(not(target_os = "macos"))]
                {
                    let _ = api;
                    log::info!("Window {} close requested", window_label);
                    app_handle.exit(0);
                }
            }
            tauri::WindowEvent::Focused(focused) => {
                if *focused {
                    // Return keyboard focus to Notion rather than the strip.
                    if let Some(webview) = orchestrator(&app_handle).and_then(|o| o.active_webview()) {
                        let _ = webview.set_focus();
                    }
                }
            }
            tauri::WindowEvent::Resized(_) => {
                if let Some(orchestrator) = orchestrator(&app_handle) {
                    orchestrator.layout(&window);
                }
                if let Some(strip) = strip(&app_handle) {
                    let fullscreen = window.is_fullscreen().unwrap_or(false);
                    let _ = strip.eval(format!(
                        "window.__lotionSetFullscreen && window.__lotionSetFullscreen({fullscreen});"
                    ));
                }
                record_bounds(&app_handle, &window, &window_label);
            }
            tauri::WindowEvent::Moved(_) => {
                record_bounds(&app_handle, &window, &window_label);
            }
            _ => {}
        });
    }

    pub fn setup_tabs(&self, app: &AppHandle<R>) -> tauri::Result<()> {
        let Some(tab_manager) = orchestrator(app) else {
            return Err(tauri::Error::AssetNotFound("TabOrchestrator state missing".into()));
        };

        let restore = app
            .try_state::<ConfigState>()
            .map(|c| c.lock().expect("config lock poisoned").restore_tabs)
            .unwrap_or(true);

        let mut restored_active = None;
        if restore {
            let saved: Vec<(String, String, bool)> = app
                .try_state::<SharedState>()
                .map(|s| {
                    let state = s.lock().expect("AppState lock poisoned");
                    state
                        .windows
                        .get(self.window.label())
                        .map(|w| {
                            w.tab_ids
                                .iter()
                                .filter_map(|id| state.tabs.get(id))
                                .map(|t| {
                                    let active = w.active_tab_id.as_deref() == Some(t.id.as_str());
                                    (t.url.clone(), t.title.clone(), active)
                                })
                                .collect()
                        })
                        .unwrap_or_default()
                })
                .unwrap_or_default();
            log::info!("WindowController: Restoring {} tabs from saved state.", saved.len());

            let any_active = saved.iter().any(|(_, _, active)| *active);
            for (i, (url, title, was_active)) in saved.into_iter().enumerate() {
                // Tabs keep their order; all start unloaded and only the
                // active one loads now (the others load when first selected).
                match tab_manager.create_lazy_tab(app, self.window.label(), &url, &title) {
                    Ok(id) => {
                        apply_zoom_to(app, &id);
                        if was_active || (!any_active && i == 0) {
                            restored_active = Some(id);
                        }
                    }
                    Err(e) => log::warn!("Skipping saved tab {}: {}", url, e),
                }
            }
        }

        if tab_manager.get_tab_ids().is_empty() {
            log::info!("WindowController: Creating initial tab for Notion: {}", NOTION_HOME);
            let id = tab_manager.create_tab(app, self.window.label(), NOTION_HOME)?;
            apply_zoom_to(app, &id);
            let _ = tab_manager.show_tab(&id);
        } else if let Some(active) = restored_active {
            let _ = tab_manager.show_tab(&active);
        }

        tabs_changed(app);
        let _ = self.window.show();
        let _ = self.window.set_focus();
        Ok(())
    }
}

fn position_is_visible<R: Runtime>(window: &Window<R>, x: f64, y: f64, width: f64) -> bool {
    let Ok(monitors) = window.available_monitors() else {
        return false;
    };
    // At least the top strip (where the user grabs the window) must be on a screen.
    monitors.iter().any(|m| {
        let scale = m.scale_factor();
        let pos = m.position().to_logical::<f64>(scale);
        let size = m.size().to_logical::<f64>(scale);
        let probe_x = x + width.min(200.0) / 2.0;
        let probe_y = y + 10.0;
        probe_x >= pos.x && probe_x < pos.x + size.width && probe_y >= pos.y && probe_y < pos.y + size.height
    })
}

fn record_bounds<R: Runtime>(app: &AppHandle<R>, window: &Window<R>, label: &str) {
    let (Ok(scale), Ok(pos), Ok(size)) = (window.scale_factor(), window.outer_position(), window.inner_size()) else {
        return;
    };
    let maximized = window.is_maximized().unwrap_or(false);
    let fullscreen = window.is_fullscreen().unwrap_or(false);
    let minimized = window.is_minimized().unwrap_or(false);
    let pos = pos.to_logical::<f64>(scale);
    let size = size.to_logical::<f64>(scale);
    persist(app, |state| {
        let w = state
            .windows
            .entry(label.to_string())
            .or_insert_with(|| crate::state::WindowState::new(label));
        w.is_maximized = maximized;
        w.is_full_screen = fullscreen;
        w.is_minimized = minimized;
        // Keep the restored (un-maximized) geometry for next launch.
        if !maximized && !fullscreen && !minimized {
            w.bounds.x = Some(pos.x);
            w.bounds.y = Some(pos.y);
            w.bounds.width = size.width;
            w.bounds.height = size.height;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_strip_remains_addressable_with_notion_child_webviews() {
        let app = tauri::test::mock_app();
        let main = WebviewWindowBuilder::new(
            &app,
            MAIN_WINDOW,
            tauri::WebviewUrl::App("index.html".into()),
        )
        .build()
        .unwrap();
        let window = main.as_ref().window();

        for label in ["notion-tab-1", "notion-tab-2"] {
            window
                .add_child(
                    tauri::webview::WebviewBuilder::new(
                        label,
                        tauri::WebviewUrl::External("https://app.notion.com/".parse().unwrap()),
                    ),
                    LogicalPosition::new(0.0, TAB_STRIP_HEIGHT),
                    LogicalSize::new(800.0, 600.0),
                )
                .unwrap();
        }

        // Reproduce the framework condition that broke all live strip updates.
        assert!(app.get_webview_window(MAIN_WINDOW).is_none());
        let strip = strip(app.handle()).expect("strip must survive child tab creation");
        assert_eq!(strip.label(), MAIN_WINDOW);
        assert_eq!(strip.window().label(), MAIN_WINDOW);
    }
}
