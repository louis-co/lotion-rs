use lotion_rs::policy::PolicyManager;
use lotion_rs::security::SecurityModule;
use lotion_rs::theming::ThemeManager;

use lotion_rs::config::{ConfigState, LotionConfig};
use lotion_rs::i18n::I18nManager;
use lotion_rs::persistence::StateStore;
use lotion_rs::spellcheck::SpellcheckManager;
use lotion_rs::state::{AppState, SharedState, TabState};
use lotion_rs::traits::TabOrchestrator;
use lotion_rs::window_controller::{self as wc, MAIN_WINDOW};
use rand::RngCore;
use std::fs::{self, File};
use std::io::{Read, Write};
#[cfg(target_family = "unix")]
use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::Manager;

const SECRET_FILE_NAME: &str = "secret_key";
const SECRET_LEN: usize = 32;

// Helper function to get or create the application secret
fn get_or_create_app_secret() -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let secret_dir = dirs::config_dir()
        .ok_or("Could not find config directory")?
        .join("lotion-rs");
    let secret_path = secret_dir.join(SECRET_FILE_NAME);

    if secret_path.exists() {
        let mut secret = vec![0u8; SECRET_LEN];
        match File::open(&secret_path).and_then(|mut f| f.read_exact(&mut secret)) {
            Ok(()) => {
                log::info!("Application secret loaded from {}", secret_path.display());
                return Ok(secret);
            }
            // A truncated/corrupt secret must not brick the app: the only
            // thing it protects is the tab/window state, which is rebuilt.
            Err(e) => log::warn!("Application secret unreadable ({}); regenerating.", e),
        }
    }

    log::info!("Generating new application secret at {}", secret_path.display());
    fs::create_dir_all(&secret_dir)?;

    let mut secret = vec![0u8; SECRET_LEN];
    rand::thread_rng().fill_bytes(&mut secret);

    let mut file = File::create(&secret_path)?;
    #[cfg(target_family = "unix")]
    {
        // Set permissions to 0o600 (read/write only for owner)
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(&secret)?;
    Ok(secret)
}

/// Only Lotion's own tab strip (a bundled local page) may drive the app over
/// IPC. Notion's remote pages get no IPC access at all.
fn is_trusted_origin<R: tauri::Runtime>(webview: &tauri::Webview<R>) -> bool {
    if webview.label() != MAIN_WINDOW {
        log::warn!("Untrusted webview '{}' attempted to invoke a command.", webview.label());
        return false;
    }
    match webview.url() {
        Ok(url) => {
            let trusted = matches!(
                (url.scheme(), url.host_str()),
                ("tauri", Some("localhost")) | ("http" | "https", Some("tauri.localhost"))
            );
            if !trusted {
                log::warn!("Untrusted origin {} attempted to invoke a command.", url);
            }
            trusted
        }
        Err(_) => {
            log::warn!("Could not determine origin for command invocation.");
            false
        }
    }
}

#[tauri::command]
fn get_window_tabs(
    webview: tauri::Webview<tauri::Wry>,
    window_id: String,
    orchestrator: tauri::State<'_, Arc<dyn lotion_rs::traits::TabOrchestrator<tauri::Wry>>>,
) -> Vec<TabState> {
    if !is_trusted_origin(&webview) || window_id != MAIN_WINDOW {
        return Vec::new();
    }
    log::debug!("tab strip requested tabs");
    orchestrator.tab_states()
}

#[tauri::command]
fn switch_tab(webview: tauri::Webview<tauri::Wry>, tab_id: String, app: tauri::AppHandle) {
    if is_trusted_origin(&webview) {
        wc::switch_tab(&app, &tab_id);
    }
}

#[tauri::command]
fn close_tab(webview: tauri::Webview<tauri::Wry>, tab_id: String, app: tauri::AppHandle) {
    if is_trusted_origin(&webview) {
        wc::close_tab(&app, &tab_id);
    }
}

// Async: creating a webview from a synchronous command deadlocks on Windows.
#[tauri::command]
async fn new_tab(webview: tauri::Webview<tauri::Wry>, app: tauri::AppHandle) {
    if is_trusted_origin(&webview) {
        wc::new_tab(&app, None);
    }
}

#[tauri::command]
fn move_tab(webview: tauri::Webview<tauri::Wry>, tab_id: String, to_index: usize, app: tauri::AppHandle) {
    if is_trusted_origin(&webview) {
        wc::move_tab(&app, &tab_id, to_index);
    }
}

#[tauri::command]
fn navigate_history(webview: tauri::Webview<tauri::Wry>, direction: i32, app: tauri::AppHandle) {
    if !is_trusted_origin(&webview) {
        return;
    }
    if direction < 0 {
        wc::go_back(&app);
    } else {
        wc::go_forward(&app);
    }
}

fn main() {
    #[cfg(target_os = "linux")]
    {
        std::env::set_var("NO_AT_BRIDGE", "1");
        std::env::set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "1");
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
        std::env::set_var("WEBKIT_USE_SINGLE_WEB_PROCESS", "1");
        std::env::set_var("WEBKIT_DISABLE_ACCESSIBILITY", "1");
        std::env::set_var("GTK_A11Y", "none");
        std::env::set_var("GIO_USE_VFS", "local");
    }

    // Set RUST_LOG only if not already set by the user
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "info");
    }
    env_logger::init();
    log::info!("Starting Lotion-rs...");

    let app_secret = get_or_create_app_secret().expect("Failed to get or create application secret");
    // Derive the state key once; every later save reuses it.
    let state_key = lotion_rs::state::derive_state_key(&app_secret);

    // Load user config
    let config = LotionConfig::load();
    log::info!(
        "Config: theme={}, restore_tabs={}, zoom={}, suspend_after={}min",
        config.active_theme,
        config.restore_tabs,
        config.zoom,
        config.suspend_inactive_tabs_after_minutes
    );
    let suspend_after = config.suspend_inactive_tabs_after_minutes;

    // Load saved state (if any)
    let app_state: SharedState = Arc::new(Mutex::new(
        AppState::load_with_key(&state_key).unwrap_or_default(),
    ));
    let state_store = Arc::new(StateStore::new(state_key));

    // Initialize Concrete Modules
    let security = Arc::new(SecurityModule::new());
    let policy = Arc::new(PolicyManager::new());
    let theming = Arc::new(ThemeManager::with_config(
        &config.active_theme,
        config.custom_css_path.clone(),
    ));
    let tab_manager = Arc::new(lotion_rs::tab_manager::TabManager::<tauri::Wry>::new(
        security.litebox.clone(),
    ));

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            wc::show_main_window(app);
        }))
        .invoke_handler(tauri::generate_handler![
            lotion_rs::i18n::get_translation,
            lotion_rs::i18n::set_locale,
            lotion_rs::spellcheck::check_spelling,
            lotion_rs::spellcheck::get_spelling_suggestions,
            get_window_tabs,
            switch_tab,
            close_tab,
            new_tab,
            move_tab,
            navigate_history,
        ])
        .setup(move |app| {
            app.manage::<Arc<dyn lotion_rs::traits::SecuritySandbox>>(security.litebox.clone());
            app.manage::<Arc<dyn lotion_rs::traits::PolicyEnforcer>>(policy);
            app.manage::<Arc<dyn lotion_rs::traits::ThemingEngine<tauri::Wry>>>(theming.clone());
            app.manage(theming);
            app.manage::<Arc<dyn lotion_rs::traits::TabOrchestrator<tauri::Wry>>>(tab_manager.clone());
            app.manage::<ConfigState>(Mutex::new(config));
            app.manage(app_state);
            app.manage(state_store);
            app.manage(I18nManager::new());
            app.manage(SpellcheckManager::new());

            let handle = app.handle().clone();

            if let Err(e) = lotion_rs::menu::create_main_menu(&handle) {
                log::error!("Failed to create menu: {}", e);
            }

            let security_state = handle
                .state::<Arc<dyn lotion_rs::traits::SecuritySandbox>>()
                .inner()
                .clone();

            let wc = lotion_rs::window_controller::WindowController::<tauri::Wry>::new(&handle, security_state)?;
            wc.setup_listeners(handle.clone());
            if let Err(e) = wc.setup_tabs(&handle) {
                log::error!("Failed to set up tabs: {}", e);
            }

            // Periodically unload long-hidden tabs so many open tabs with large
            // databases don't exhaust memory (WebKit kills the web process
            // under memory pressure, which shows up as a blank tab).
            if suspend_after > 0 {
                let max_idle = Duration::from_secs(suspend_after * 60);
                let timer_handle = handle.clone();
                std::thread::Builder::new()
                    .name("lotion-tab-suspender".into())
                    .spawn(move || loop {
                        std::thread::sleep(Duration::from_secs(60));
                        let tabs = tab_manager.clone();
                        let _ = timer_handle.run_on_main_thread(move || tabs.suspend_idle_tabs(max_idle));
                    })?;
            }

            #[cfg(debug_assertions)]
            lotion_rs::diagnostics::start(&handle);
            #[cfg(debug_assertions)]
            lotion_rs::diagnostics::start_selftest(&handle);
            #[cfg(debug_assertions)]
            lotion_rs::diagnostics::start_scroll_bench(&handle);

            log::info!("Tauri background layer initialized.");
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app, event| match event {
        // Dock icon clicked while the window is hidden (macOS).
        #[cfg(target_os = "macos")]
        tauri::RunEvent::Reopen { .. } => wc::show_main_window(app),
        tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit => {
            if let Some(store) = app.try_state::<Arc<StateStore>>() {
                store.shutdown();
            }
        }
        _ => {}
    });
}
