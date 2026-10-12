use crate::window_controller as wc;
use std::sync::Arc;
use tauri::{
    menu::{AboutMetadata, MenuBuilder, MenuItem, PredefinedMenuItem, SubmenuBuilder},
    AppHandle, Manager, Runtime,
};

fn item<R: Runtime>(
    app: &AppHandle<R>,
    id: &str,
    label: &str,
    accelerator: Option<&str>,
) -> tauri::Result<MenuItem<R>> {
    MenuItem::with_id(app, id, label, true, accelerator)
}

pub fn create_main_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let pkg_info = app.package_info();

    // App menu (macOS shows it under the app name; elsewhere it is "Lotion").
    let app_menu = SubmenuBuilder::new(app, "Lotion")
        .about(Some(AboutMetadata {
            name: Some("Lotion".to_string()),
            version: Some(pkg_info.version.to_string()),
            ..Default::default()
        }))
        .separator()
        .item(&item(app, "preferences", "Settings…", Some("CmdOrCtrl+,"))?)
        .separator()
        .services()
        .separator()
        .hide()
        .hide_others()
        .show_all()
        .separator()
        .quit()
        .build()?;

    let file_menu = SubmenuBuilder::new(app, "File")
        .item(&item(app, "tab_new", "New Tab", Some("CmdOrCtrl+T"))?)
        .item(&item(
            app,
            "tab_reopen",
            "Reopen Closed Tab",
            Some("CmdOrCtrl+Shift+T"),
        )?)
        .separator()
        .item(&item(app, "tab_close", "Close Tab", Some("CmdOrCtrl+W"))?)
        .item(&item(
            app,
            "window_close",
            "Close Window",
            Some("CmdOrCtrl+Shift+W"),
        )?)
        .build()?;

    let edit_menu = SubmenuBuilder::new(app, "Edit")
        .undo()
        .redo()
        .separator()
        .cut()
        .copy()
        .paste()
        .select_all()
        .build()?;

    let theme_submenu = SubmenuBuilder::new(app, "Dark Mode Theme")
        .item(&item(app, "theme_default", "Notion (Default)", None)?)
        .item(&item(app, "theme_dracula", "Dracula", None)?)
        .item(&item(app, "theme_nord", "Nord", None)?)
        .build()?;

    let view_menu = SubmenuBuilder::new(app, "View")
        .item(&item(app, "reload", "Reload", Some("CmdOrCtrl+R"))?)
        .item(&item(
            app,
            "reload_force",
            "Force Reload",
            Some("CmdOrCtrl+Shift+R"),
        )?)
        .separator()
        .item(&item(
            app,
            "zoom_reset",
            "Actual Size",
            Some("CmdOrCtrl+0"),
        )?)
        .item(&item(app, "zoom_in", "Zoom In", Some("CmdOrCtrl+Plus"))?)
        .item(&item(app, "zoom_in_alt", "Zoom In", Some("CmdOrCtrl+="))?)
        .item(&item(app, "zoom_out", "Zoom Out", Some("CmdOrCtrl+-"))?)
        .separator()
        .item(&theme_submenu)
        .separator()
        .item(&PredefinedMenuItem::fullscreen(app, None)?)
        .item(&item(
            app,
            "toggle_dev_tools",
            "Developer Tools",
            Some("Alt+CmdOrCtrl+I"),
        )?)
        .build()?;

    let history_menu = SubmenuBuilder::new(app, "History")
        .item(&item(app, "nav_back", "Back", Some("CmdOrCtrl+["))?)
        .item(&item(app, "nav_forward", "Forward", Some("CmdOrCtrl+]"))?)
        .separator()
        .item(&item(app, "nav_home", "Home", Some("CmdOrCtrl+Shift+H"))?)
        .build()?;

    let mut window_menu = SubmenuBuilder::new(app, "Window")
        .minimize()
        .maximize()
        .separator()
        .item(&item(app, "tab_next", "Select Next Tab", Some("Ctrl+Tab"))?)
        .item(&item(
            app,
            "tab_prev",
            "Select Previous Tab",
            Some("Ctrl+Shift+Tab"),
        )?)
        .item(&item(
            app,
            "tab_next_alt",
            "Select Next Tab",
            Some("CmdOrCtrl+Shift+]"),
        )?)
        .item(&item(
            app,
            "tab_prev_alt",
            "Select Previous Tab",
            Some("CmdOrCtrl+Shift+["),
        )?)
        .item(&item(
            app,
            "tab_next_alt2",
            "Select Next Tab",
            Some("Alt+CmdOrCtrl+Right"),
        )?)
        .item(&item(
            app,
            "tab_prev_alt2",
            "Select Previous Tab",
            Some("Alt+CmdOrCtrl+Left"),
        )?)
        .separator();
    for n in 1..=9 {
        let label = if n == 9 {
            "Select Last Tab".to_string()
        } else {
            format!("Select Tab {n}")
        };
        window_menu = window_menu.item(&item(
            app,
            &format!("tab_select_{n}"),
            &label,
            Some(&format!("CmdOrCtrl+{n}")),
        )?);
    }
    let window_menu = window_menu
        .separator()
        .item(&item(
            app,
            "window_show",
            "Notion",
            Some("CmdOrCtrl+Alt+0"),
        )?)
        .build()?;

    let help_menu = SubmenuBuilder::new(app, "Help")
        .item(&item(app, "help_notion", "Notion Help & Support", None)?)
        .item(&item(app, "help_lotion", "Lotion on GitHub", None)?)
        .build()?;

    let menu = MenuBuilder::new(app)
        .item(&app_menu)
        .item(&file_menu)
        .item(&edit_menu)
        .item(&view_menu)
        .item(&history_menu)
        .item(&window_menu)
        .item(&help_menu)
        .build()?;

    app.set_menu(menu)?;

    app.on_menu_event(move |app_handle, event| {
        let id = event.id().as_ref();
        match id {
            "tab_new" => wc::new_tab(app_handle, None),
            "tab_reopen" => wc::reopen_closed_tab(app_handle),
            "tab_close" => wc::close_active_tab(app_handle),
            "window_close" => wc::close_main_window(app_handle),
            "window_show" => wc::show_main_window(app_handle),
            "tab_next" | "tab_next_alt" | "tab_next_alt2" => wc::select_relative_tab(app_handle, 1),
            "tab_prev" | "tab_prev_alt" | "tab_prev_alt2" => {
                wc::select_relative_tab(app_handle, -1)
            }
            "nav_back" => wc::go_back(app_handle),
            "nav_forward" => wc::go_forward(app_handle),
            "nav_home" => wc::go_home(app_handle),
            "reload" => wc::reload_active(app_handle),
            "reload_force" => wc::force_reload_active(app_handle),
            "zoom_in" | "zoom_in_alt" => wc::zoom(app_handle, 1),
            "zoom_out" => wc::zoom(app_handle, -1),
            "zoom_reset" => wc::zoom(app_handle, 0),
            "toggle_dev_tools" => wc::open_devtools(app_handle),
            "preferences" => open_config_file(),
            "help_notion" => wc::new_tab(app_handle, Some("https://www.notion.so/help")),
            "help_lotion" => {
                use tauri_plugin_shell::ShellExt;
                #[allow(deprecated)]
                let _ = app_handle
                    .shell()
                    .open("https://github.com/diegoakanotoperator/lotion-rs", None);
            }
            tab if tab.starts_with("tab_select_") => {
                if let Ok(n) = tab.trim_start_matches("tab_select_").parse::<usize>() {
                    wc::select_tab_number(app_handle, n);
                }
            }
            theme_id if theme_id.starts_with("theme_") => {
                let theme_name = theme_id.trim_start_matches("theme_").to_string();
                log::info!("Menu: Switch theme to {}", theme_name);
                switch_theme(app_handle, &theme_name);
            }
            _ => {}
        }
    });

    Ok(())
}

fn switch_theme<R: Runtime>(app_handle: &AppHandle<R>, theme_name: &str) {
    let Some(theming) = app_handle.try_state::<Arc<dyn crate::traits::ThemingEngine<R>>>() else {
        return;
    };
    theming.set_active_theme(theme_name);

    if let Some(orchestrator) = app_handle.try_state::<Arc<dyn crate::traits::TabOrchestrator<R>>>()
    {
        for tab_id in orchestrator.get_tab_ids() {
            let _ = orchestrator.inject_theme_into_tab(app_handle, &tab_id, theme_name);
        }
    }

    if let Some(config) = app_handle.try_state::<crate::config::ConfigState>() {
        let mut config = config.lock().expect("config lock poisoned");
        config.active_theme = theme_name.to_string();
        let _ = config.save();
    }
}

/// "Settings…" opens the TOML config in the user's default editor.
fn open_config_file() {
    let path = crate::config::LotionConfig::config_file_path();
    log::info!("Opening config file {}", path.display());
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open")
        .arg("-t")
        .arg(&path)
        .spawn();
    #[cfg(target_os = "linux")]
    let result = std::process::Command::new("xdg-open").arg(&path).spawn();
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("notepad").arg(&path).spawn();
    if let Err(e) = result {
        log::warn!("Failed to open config file: {}", e);
    }
}
