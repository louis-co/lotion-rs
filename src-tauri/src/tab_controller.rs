use crate::litebox::LiteBox;
use crate::traits::PolicyEnforcer;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
#[cfg(not(target_os = "macos"))]
use tauri::webview::DownloadEvent;
use tauri::webview::{NewWindowResponse, PageLoadEvent, Webview};
use tauri::{AppHandle, Manager, Runtime, Url, WebviewBuilder, WebviewUrl};

// notion.so now redirects to the marketing site even with an existing session.
// Use the application origin for startup, fresh tabs, and the Home action.
pub const NOTION_HOME: &str = "https://app.notion.com/";

fn notion_home_url(url: &str) -> &str {
    let Ok(parsed) = Url::parse(url) else {
        return url;
    };
    if parsed.scheme() == "https"
        && matches!(
            parsed.host_str(),
            Some("notion.so" | "www.notion.so" | "notion.com" | "www.notion.com")
        )
        && parsed.path() == "/"
        && parsed.query().is_none()
        && parsed.fragment().is_none()
    {
        NOTION_HOME
    } else {
        url
    }
}

/// Height of the native tab strip (logical px), matching Notion desktop's
/// 36px tab bar. Notion content sits below it.
pub const TAB_STRIP_HEIGHT: f64 = 36.0;

pub struct TabController<R: Runtime> {
    pub tab_id: String,
    pub window_id: String,
    pub webview: Webview<R>,
    pub title: Mutex<String>,
    pub url: Mutex<String>,
    /// When the tab was last shown; drives memory-saving suspension.
    pub last_active: Mutex<Instant>,
    /// Whether the tab's Notion document has been unloaded to save memory.
    pub suspended: AtomicBool,
    /// Hidden tabs are not resized during live window resizes (relayouting a
    /// large database in a tab nobody can see is wasted work); they are laid
    /// out once when shown.
    pub needs_layout: AtomicBool,
}

impl<R: Runtime> TabController<R> {
    pub fn new(
        app: &AppHandle<R>,
        window_id: &str,
        tab_id: String,
        url_str: &str,
        initial_title: &str,
        lazy: bool,
        _litebox: Arc<LiteBox>,
    ) -> tauri::Result<Self> {
        // Also migrate default-home URLs retained by older saved/closed tabs.
        // Real page URLs, views, and public notion.site pages keep their URL.
        let url_str = notion_home_url(url_str);
        let policy = app.state::<Arc<dyn PolicyEnforcer>>().inner().clone();

        // Zero-Trust Enforcement: Validate URL before creation
        if !policy.validate_url(url_str) {
            return Err(tauri::Error::AssetNotFound(format!(
                "Zero-Trust Policy Blocked: {}",
                url_str
            )));
        }

        let window = app
            .get_window(window_id)
            .ok_or(tauri::Error::AssetNotFound(format!(
                "Window {} not found",
                window_id
            )))?;

        let url = url_str
            .parse::<Url>()
            .map_err(|e| tauri::Error::AssetNotFound(e.to_string()))?;

        // Lazy tabs (restored background tabs) start on an empty document and
        // load Notion the first time they're shown, like Notion desktop: only
        // the active tab pays the startup cost.
        let initial_url = if lazy {
            "about:blank".parse::<Url>().expect("valid url")
        } else {
            url.clone()
        };
        let mut webview_builder =
            create_secure_webview_builder(app, &tab_id, &initial_url, window_id, policy.clone());
        // Paint Notion's own page colour before the first frame instead of
        // flashing white in dark mode.
        let dark = matches!(window.theme(), Ok(tauri::Theme::Dark));
        webview_builder = webview_builder.background_color(if dark {
            tauri::window::Color(0x19, 0x19, 0x19, 0xff)
        } else {
            tauri::window::Color(0xff, 0xff, 0xff, 0xff)
        });

        let (position, size) = crate::window_controller::content_bounds(&window)?;
        let webview = window.add_child(webview_builder, position, size)?;
        // Created hidden; the orchestrator decides which tab is visible.
        let _ = webview.hide();

        log::info!("Created tab webview: {} in window: {}", tab_id, window_id);

        Ok(Self {
            tab_id,
            window_id: window_id.to_string(),
            webview,
            title: Mutex::new(initial_title.to_string()),
            url: Mutex::new(url_str.to_string()),
            last_active: Mutex::new(Instant::now()),
            suspended: AtomicBool::new(lazy),
            needs_layout: AtomicBool::new(false),
        })
    }

    pub fn load_url(&self, app: &AppHandle<R>, url: &str) -> tauri::Result<()> {
        let policy = app.state::<Arc<dyn PolicyEnforcer>>();

        // Zero-Trust Enforcement: Validate URL before navigation
        if !policy.validate_url(url) {
            log::warn!("Zero-Trust Policy Blocked navigation to: {}", url);
            return Ok(()); // Fail closed (don't navigate)
        }

        log::info!("Tab {}: Loading URL {}", self.tab_id, url);
        let url = url
            .parse::<Url>()
            .map_err(|e| tauri::Error::AssetNotFound(e.to_string()))?;
        self.webview.navigate(url)?;
        Ok(())
    }

    pub fn current_url(&self) -> String {
        self.url
            .lock()
            .expect("TabController url lock poisoned")
            .clone()
    }

    pub fn current_title(&self) -> String {
        self.title
            .lock()
            .expect("TabController title lock poisoned")
            .clone()
    }

    pub fn show(&self) -> tauri::Result<()> {
        *self
            .last_active
            .lock()
            .expect("TabController lock poisoned") = Instant::now();
        if self.needs_layout.swap(false, Ordering::AcqRel) {
            let window = self.webview.window();
            let (position, size) = crate::window_controller::content_bounds(&window)?;
            self.webview.set_bounds(tauri::Rect {
                position: position.into(),
                size: size.into(),
            })?;
        }
        if self.suspended.swap(false, Ordering::AcqRel) {
            log::info!("Tab {}: resuming suspended tab", self.tab_id);
            if let Ok(url) = self.current_url().parse::<Url>() {
                self.webview.navigate(url)?;
            }
        }
        self.webview.show()?;
        self.webview.set_focus()?;
        Ok(())
    }

    pub fn hide(&self) -> tauri::Result<()> {
        *self
            .last_active
            .lock()
            .expect("TabController lock poisoned") = Instant::now();
        self.webview.hide()?;
        Ok(())
    }

    /// Unload the Notion document of a hidden tab to release its memory. The
    /// tab's URL and title are kept; it reloads when shown again.
    pub fn suspend(&self) -> tauri::Result<()> {
        if self.suspended.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        log::info!("Tab {}: suspending inactive tab", self.tab_id);
        self.webview
            .navigate("about:blank".parse::<Url>().expect("valid url"))?;
        Ok(())
    }

    pub fn destroy(&self) -> tauri::Result<()> {
        log::info!("Destroying tab: {}", self.tab_id);
        self.webview.close()?;
        Ok(())
    }
}

/// Safari's version, so WKWebView presents itself as the browser it is.
///
/// WKWebView's default user agent omits the `Version/x Safari/y` tokens, which
/// makes web apps (Notion included) treat it as an unknown embedded browser and
/// take slower/compatibility code paths.
#[cfg(target_os = "macos")]
fn safari_user_agent() -> &'static str {
    use std::sync::OnceLock;
    static UA: OnceLock<String> = OnceLock::new();
    UA.get_or_init(|| {
        let version = std::fs::read_to_string("/Applications/Safari.app/Contents/Info.plist")
            .ok()
            .and_then(|plist| {
                let key = plist.find("<key>CFBundleShortVersionString</key>")?;
                let rest = &plist[key..];
                let start = rest.find("<string>")? + "<string>".len();
                let end = rest[start..].find("</string>")? + start;
                Some(rest[start..end].trim().to_string())
            })
            .filter(|v| !v.is_empty() && v.chars().all(|c| c.is_ascii_digit() || c == '.'))
            .unwrap_or_else(|| "18.0".to_string());
        // `WantsServiceWorker` is the token Notion desktop appends so the web
        // app serves its assets from a service worker cache.
        format!(
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/{} Safari/605.1.15 WantsServiceWorker",
            version
        )
    })
}

/// Opens a new Notion tab next to the current one and shows it.
pub fn open_tab_with_url<R: Runtime>(app: &AppHandle<R>, window_id: &str, url: &str) {
    if let Some(orchestrator) = app.try_state::<Arc<dyn crate::traits::TabOrchestrator<R>>>() {
        match orchestrator.inner().create_tab(app, window_id, url) {
            Ok(id) => {
                let _ = orchestrator.inner().show_tab(&id);
                crate::window_controller::tabs_changed(app);
            }
            Err(e) => log::error!("Zero-Trust: Failed to route popup into managed tab: {}", e),
        }
    } else {
        log::error!("Zero-Trust: Cannot spawn tab securely. TabOrchestrator missing from state.");
    }
}

fn open_in_system_browser<R: Runtime>(app: &AppHandle<R>, policy: &dyn PolicyEnforcer, url: &str) {
    if policy.validate_external_link(url) {
        log::info!(
            "Zero-Trust: Opening validated external link in default browser: {}",
            url
        );
        use tauri_plugin_shell::ShellExt;
        #[allow(deprecated)]
        let _ = app.shell().open(url.to_string(), None);
    } else {
        log::warn!("Zero-Trust: BLOCKED unauthorized external link: {}", url);
    }
}

/// Real popup window for flows that need `window.opener` (Google / Apple
/// sign-in, `window.open()` followed by a location change). It is confined to
/// Notion + OAuth providers; anything else goes to the system browser and the
/// popup closes itself.
fn spawn_opener_popup<R: Runtime>(
    app: &AppHandle<R>,
    policy: Arc<dyn PolicyEnforcer>,
    url: &Url,
    features: tauri::webview::NewWindowFeatures,
) -> NewWindowResponse<R> {
    let label = format!("popup-{}", uuid::Uuid::new_v4());
    let nav_policy = policy.clone();
    let load_app = app.clone();
    let load_label = label.clone();
    #[allow(unused_mut)]
    let mut builder =
        tauri::WebviewWindowBuilder::new(app, &label, WebviewUrl::External(url.clone()))
            .title("Notion")
            .inner_size(520.0, 680.0)
            .window_features(features)
            .on_navigation(move |url| nav_policy.validate_frame_url(url.as_str()))
            .on_page_load(move |_window, payload| {
                let url_str = payload.url().as_str();
                if payload.event() != PageLoadEvent::Started
                    || url_str == "about:blank"
                    || policy.validate_url(url_str)
                {
                    return;
                }
                // Main frame left Notion/OAuth: this was an external link opened
                // via window.open(); hand it to the browser.
                open_in_system_browser(&load_app, policy.as_ref(), url_str);
                if let Some(w) = load_app.get_webview_window(&load_label) {
                    let _ = w.close();
                }
            });
    #[cfg(target_os = "macos")]
    {
        builder = builder.user_agent(safari_user_agent());
    }
    match builder.build() {
        Ok(window) => NewWindowResponse::Create { window },
        Err(e) => {
            log::error!("Failed to create popup window: {}", e);
            NewWindowResponse::Deny
        }
    }
}

/// A centralized factory for creating WebviewBuilders with guaranteed security listeners.
/// Ensures all created webviews (tabs or popups) enforce on_navigation and on_new_window policies.
pub fn create_secure_webview_builder<R: Runtime>(
    app: &AppHandle<R>,
    label: &str,
    url: &Url,
    window_id: &str,
    policy: Arc<dyn PolicyEnforcer>,
) -> WebviewBuilder<R> {
    let theming = app
        .try_state::<Arc<crate::theming::ThemeManager>>()
        .map(|t| t.initialization_script())
        .unwrap_or_default();

    let mut webview_builder = WebviewBuilder::new(label, WebviewUrl::External(url.clone()))
        // Hidden tabs keep their realtime connection but run timers at a
        // reduced rate instead of being frozen or running at full speed.
        .background_throttling(tauri::utils::config::BackgroundThrottlingPolicy::Throttle);
    #[cfg(target_os = "macos")]
    {
        webview_builder = webview_builder.user_agent(safari_user_agent());
    }
    if !theming.is_empty() {
        webview_builder = webview_builder.initialization_script(theming);
    }

    let nav_policy = policy.clone();
    let load_app = app.clone();
    let load_policy = policy.clone();
    let popup_app = app.clone();
    let popup_policy = policy.clone();
    let popup_window_id = window_id.to_string();
    let title_app = app.clone();
    let download_app = app.clone();
    #[cfg(target_os = "macos")]
    let downloads = crate::downloads::DownloadManager::default();

    webview_builder
        // WebKit asks this for main-frame *and* sub-frame navigations without
        // telling us which, so it must allow embeds (YouTube, Figma, Loom...)
        // and Notion's internal about:blank frames. Main-frame confinement to
        // Notion happens in on_page_load below.
        .on_navigation(move |url| {
            let allowed = nav_policy.validate_frame_url(url.as_str());
            if !allowed {
                log::warn!("Zero-Trust: BLOCKED navigation to: {}", url.as_str());
            }
            allowed
        })
        .on_page_load(move |webview, payload| {
            let url = payload.url();
            let url_str = url.as_str();
            // Suspended tabs park on about:blank; that is expected.
            if url_str == "about:blank" {
                return;
            }
            if load_policy.validate_url(url_str) {
                if payload.event() == PageLoadEvent::Finished {
                    crate::window_controller::tab_url_changed(&load_app, webview.label(), url_str);
                }
                return;
            }
            if payload.event() != PageLoadEvent::Started {
                return;
            }
            // The *main frame* left Notion (e.g. a checkout or help-center
            // redirect). Hand the URL to the system browser and put the tab
            // back where it was.
            log::warn!(
                "Zero-Trust: main frame left Notion for {}, redirecting to browser",
                url_str
            );
            open_in_system_browser(&load_app, load_policy.as_ref(), url_str);
            let back = load_app
                .try_state::<Arc<dyn crate::traits::TabOrchestrator<R>>>()
                .and_then(|o| o.tab_url(webview.label()))
                .filter(|u| load_policy.validate_url(u))
                .unwrap_or_else(|| NOTION_HOME.to_string());
            if let Ok(back) = back.parse::<Url>() {
                let _ = webview.navigate(back);
            }
        })
        .on_document_title_changed(move |webview, title| {
            crate::window_controller::tab_title_changed(&title_app, &webview, title);
        })
        .on_download(move |_webview, event| {
            #[cfg(target_os = "macos")]
            {
                crate::downloads::handle_download(&download_app, &downloads, event)
            }
            #[cfg(not(target_os = "macos"))]
            {
                match event {
                    DownloadEvent::Requested { url, destination } => {
                        log::info!("Download requested: {} -> {}", url, destination.display());
                    }
                    DownloadEvent::Finished { path, success, .. } => {
                        let name = path
                            .as_ref()
                            .and_then(|p| p.file_name())
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        log::info!("Download finished (success: {}): {}", success, name);
                        let msg = if success {
                            format!("Downloaded {} to Downloads", name)
                        } else {
                            "Download failed".to_string()
                        };
                        crate::window_controller::toast(&download_app, &msg);
                    }
                    _ => {}
                }
                true
            }
        })
        .on_new_window(move |url, features| {
            let url_str = url.as_str();
            let is_oauth_or_blank = url_str == "about:blank"
                || (popup_policy.validate_url(url_str)
                    && !popup_policy.should_route_popup_to_system_browser(url_str)
                    && url
                        .host_str()
                        .map(|h| !h.ends_with("notion.so") && !h.ends_with("notion.com"))
                        .unwrap_or(false));

            if is_oauth_or_blank {
                return spawn_opener_popup(&popup_app, popup_policy.clone(), &url, features);
            }
            if popup_policy.validate_url(url_str) {
                // Notion page opened "in new tab".
                let app = popup_app.clone();
                let window_id = popup_window_id.clone();
                let url = url_str.to_string();
                // Defer: creating a webview from inside WebKit's delegate
                // callback re-enters the event loop.
                let _ = popup_app.run_on_main_thread(move || {
                    open_tab_with_url(&app, &window_id, &url);
                });
            } else {
                open_in_system_browser(&popup_app, popup_policy.as_ref(), url_str);
            }
            NewWindowResponse::Deny
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_home_tabs_reopen_at_the_application_origin() {
        for url in [
            "https://notion.so",
            "https://www.notion.so/",
            "https://notion.com/",
            "https://www.notion.com/",
        ] {
            assert_eq!(notion_home_url(url), "https://app.notion.com/");
        }
    }

    #[test]
    fn home_migration_preserves_pages_views_and_untrusted_urls() {
        for url in [
            "https://app.notion.com/p/example-page",
            "https://www.notion.so/page-123?v=view-456",
            "https://notion.com/product",
            "https://notion.so/?v=view-456",
            "https://notion.so/#page",
            "https://acme.notion.site/",
            "https://notion.so.evil.com/",
            "http://notion.so/",
            "not a URL",
        ] {
            assert_eq!(notion_home_url(url), url);
        }
    }
}
