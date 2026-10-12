use tauri::{AppHandle, Runtime};

/// Interface for security sandboxing
pub trait SecuritySandbox: Send + Sync {
    fn initialize(&self);
    fn get_fd_count(&self) -> usize;
}

/// Interface for tab navigation and orchestration
pub trait TabOrchestrator<R: Runtime>: Send + Sync {
    /// Create a hidden tab, inserted after the active tab.
    fn create_tab(&self, app: &AppHandle<R>, window_id: &str, url: &str) -> tauri::Result<String>;
    /// Create a hidden, not-yet-loaded tab (appended); it loads `url` when first shown.
    fn create_lazy_tab(
        &self,
        app: &AppHandle<R>,
        window_id: &str,
        url: &str,
        title: &str,
    ) -> tauri::Result<String>;
    fn destroy_tab(&self, tab_id: &str) -> tauri::Result<()>;
    fn show_tab(&self, tab_id: &str) -> tauri::Result<()>;
    /// Tab ids in display order.
    fn get_tab_ids(&self) -> Vec<String>;
    fn active_tab_id(&self) -> Option<String>;
    fn active_webview(&self) -> Option<tauri::Webview<R>>;
    /// Display-ordered snapshot of all tabs.
    fn tab_states(&self) -> Vec<crate::state::TabState>;
    fn tab_url(&self, tab_id: &str) -> Option<String>;
    /// Record a title/url change reported by a tab's webview.
    fn update_tab_meta(&self, tab_id: &str, title: Option<String>, url: Option<String>) -> bool;
    /// Pop the most recently closed tab's URL.
    fn pop_closed_tab(&self) -> Option<String>;
    /// Move a tab to a new display index.
    fn move_tab(&self, tab_id: &str, to_index: usize);
    /// Resize the visible tab to the window's content area; hidden tabs are
    /// marked and resized lazily when shown.
    fn layout(&self, window: &tauri::Window<R>);
    /// Unload hidden tabs that have been inactive for longer than `max_idle`,
    /// always keeping the most recently used ones loaded.
    fn suspend_idle_tabs(&self, max_idle: std::time::Duration);
    fn inject_theme_into_tab(
        &self,
        app: &AppHandle<R>,
        tab_id: &str,
        theme_name: &str,
    ) -> tauri::Result<()>;
}

/// Interface for window management
pub trait WindowProvider<R: Runtime>: Send + Sync {
    fn create_main_window(&self, app: &AppHandle<R>) -> tauri::Result<()>;
}

/// Interface for enforcing Zero-Trust Manifesto policies
pub trait PolicyEnforcer: Send + Sync {
    /// Returns true if navigation to the URL is allowed by the manifesto
    fn validate_url(&self, url: &str) -> bool;
    /// Policy on whether telemetry is allowed (defaults to false per Manifesto)
    fn telemetry_allowed(&self) -> bool;
    /// Returns true if an external link is safe to open in the system browser
    fn validate_external_link(&self, url: &str) -> bool;
    /// Returns true if a popup request (like OAuth) should be routed to the system browser
    fn should_route_popup_to_system_browser(&self, url: &str) -> bool;
    /// Returns true if a navigation that may target a sub-frame (embeds such as
    /// YouTube/Figma/Loom, or Notion's internal `about:blank` frames) is allowed.
    ///
    /// WebKit reports iframe navigations through the same policy hook as
    /// top-level ones, so this is the permissive check; top-level navigations
    /// are additionally confined to Notion via [`PolicyEnforcer::validate_url`]
    /// once the main frame commits.
    fn validate_frame_url(&self, url: &str) -> bool;
}

/// Interface for the CSS/JS Theming Engine
pub trait ThemingEngine<R: Runtime>: Send + Sync {
    /// Returns the CSS string to be injected for a given theme name
    fn get_theme_css(&self, theme_name: &str) -> String;
    /// Returns the CSS to apply custom user overrides
    fn get_custom_css(&self) -> String;
    /// Injects the current theme into a webview
    fn inject_theme(&self, webview: &tauri::Webview<R>, theme_name: &str);
    /// Set the active theme at runtime
    fn set_active_theme(&self, name: &str);
    /// Get the currently active theme name
    fn get_active_theme(&self) -> String;
}
