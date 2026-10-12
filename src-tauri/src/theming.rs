use crate::traits::ThemingEngine;
use std::path::PathBuf;
use std::sync::RwLock;
use tauri::{Runtime, Webview};

/// Built-in themes. Each only applies while Notion itself is in dark mode, so
/// picking a theme never fights Notion's light appearance.
const THEMES: &[(&str, &str)] = &[
    (
        "dracula",
        r#"
.notion-body.dark, .notion-body.dark .notion-frame, .notion-body.dark .notion-peek-renderer,
.notion-dark-theme .notion-frame { background: #282a36 !important; }
.notion-body.dark .notion-sidebar, .notion-body.dark .notion-topbar,
.notion-dark-theme .notion-sidebar, .notion-dark-theme .notion-topbar { background: #21222c !important; }
"#,
    ),
    (
        "nord",
        r#"
.notion-body.dark, .notion-body.dark .notion-frame, .notion-body.dark .notion-peek-renderer,
.notion-dark-theme .notion-frame { background: #2e3440 !important; }
.notion-body.dark .notion-sidebar, .notion-body.dark .notion-topbar,
.notion-dark-theme .notion-sidebar, .notion-dark-theme .notion-topbar { background: #3b4252 !important; }
"#,
    ),
];

pub struct ThemeManager {
    active_theme: RwLock<String>,
    custom_css_path: RwLock<Option<PathBuf>>,
}

impl Default for ThemeManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ThemeManager {
    pub fn new() -> Self {
        Self {
            active_theme: RwLock::new("default".to_string()),
            custom_css_path: RwLock::new(None),
        }
    }

    pub fn with_config(theme: &str, css_path: Option<PathBuf>) -> Self {
        Self {
            active_theme: RwLock::new(theme.to_string()),
            custom_css_path: RwLock::new(css_path),
        }
    }

    fn theme_css(theme_name: &str) -> &'static str {
        THEMES
            .iter()
            .find(|(name, _)| *name == theme_name)
            .map(|(_, css)| *css)
            .unwrap_or("")
    }

    fn read_custom_css(&self) -> Option<String> {
        let path_guard = self
            .custom_css_path
            .read()
            .expect("ThemeManager: custom_css_path read lock poisoned");
        let path = path_guard.as_ref()?;
        match std::fs::read_to_string(path) {
            Ok(css) => {
                log::info!("Loaded custom CSS from {}", path.display());
                Some(css)
            }
            Err(e) => {
                log::warn!("Failed to read custom CSS: {}", e);
                None
            }
        }
    }

    /// JS that installs (or replaces) a `<style>` element. CSS is passed as a
    /// JSON string literal, so arbitrary content cannot break out of the script.
    fn style_script(id: &str, css: &str) -> String {
        let css = serde_json::to_string(css).unwrap_or_else(|_| "\"\"".into());
        format!(
            r#"(function(){{
  var id={id:?}, css={css};
  var el=document.getElementById(id);
  if(!css){{ if(el) el.remove(); return; }}
  if(!el){{ el=document.createElement('style'); el.id=id; (document.head||document.documentElement).appendChild(el); }}
  if(el.textContent!==css) el.textContent=css;
}})();"#
        )
    }

    /// Script registered as a document-start user script on every Notion tab.
    /// Unlike `eval`, it survives reloads and in-app navigations. A theme
    /// picked at runtime is remembered per tab in `sessionStorage`, so reloads
    /// keep it; on restart the persisted config value applies again.
    pub fn initialization_script(&self) -> String {
        let theme = <Self as ThemingEngine<tauri::Wry>>::get_active_theme(self);
        let themes: serde_json::Map<String, serde_json::Value> = THEMES
            .iter()
            .map(|(name, css)| (name.to_string(), serde_json::Value::from(*css)))
            .collect();
        let mut script = format!(
            r#"(function(){{
  var themes={themes}, t={theme};
  try {{ t = sessionStorage.getItem('lotion:theme') || t; }} catch (e) {{}}
  window.__lotionApplyTheme = function(name){{
    try {{ sessionStorage.setItem('lotion:theme', name); }} catch (e) {{}}
    var css = themes[name] || '', el = document.getElementById('lotion-theme-style');
    if (!css) {{ if (el) el.remove(); return; }}
    if (!el) {{ el = document.createElement('style'); el.id = 'lotion-theme-style'; (document.head || document.documentElement).appendChild(el); }}
    if (el.textContent !== css) el.textContent = css;
  }};
  window.__lotionApplyTheme(t);
}})();"#,
            themes = serde_json::Value::Object(themes),
            theme = serde_json::Value::from(theme),
        );
        if let Some(custom) = self.read_custom_css() {
            script.push_str(&Self::style_script("lotion-custom-style", &custom));
        }
        script
    }
}

impl<R: Runtime> ThemingEngine<R> for ThemeManager {
    fn get_theme_css(&self, theme_name: &str) -> String {
        Self::style_script("lotion-theme-style", Self::theme_css(theme_name))
    }

    fn get_custom_css(&self) -> String {
        self.read_custom_css()
            .map(|css| Self::style_script("lotion-custom-style", &css))
            .unwrap_or_default()
    }

    fn inject_theme(&self, webview: &Webview<R>, theme_name: &str) {
        // Live update for the loaded document; the initialization script
        // re-applies it after reloads.
        let name = serde_json::to_string(theme_name).unwrap_or_else(|_| "\"default\"".into());
        let _ = webview.eval(format!(
            "window.__lotionApplyTheme ? window.__lotionApplyTheme({name}) : void 0;"
        ));
    }

    fn set_active_theme(&self, name: &str) {
        let mut theme = self
            .active_theme
            .write()
            .expect("ThemeManager: active_theme write lock poisoned");
        *theme = name.to_string();
        log::info!("Active theme changed to: {}", name);
    }

    fn get_active_theme(&self) -> String {
        self.active_theme
            .read()
            .expect("ThemeManager: active_theme read lock poisoned")
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn style_script_escapes_css() {
        let script = ThemeManager::style_script("x", "a{content:\"`</script>\\\"}");
        assert!(script.contains(r#"css="a{content:\"`</script>\\\"}""#));
    }

    #[test]
    fn unknown_theme_is_empty() {
        assert_eq!(ThemeManager::theme_css("default"), "");
        assert!(!ThemeManager::theme_css("dracula").is_empty());
    }
}
