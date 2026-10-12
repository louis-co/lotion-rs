use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::sync::Mutex;
use tauri::{AppHandle, Manager, State};

pub struct I18nManager {
    translations: Mutex<HashMap<String, String>>,
    locale: Mutex<String>,
}

impl Default for I18nManager {
    fn default() -> Self {
        Self::new()
    }
}

impl I18nManager {
    pub fn new() -> Self {
        Self {
            translations: Mutex::new(HashMap::new()),
            locale: Mutex::new("en_US".to_string()),
        }
    }

    pub fn load_locale(&self, app: &AppHandle, locale: &str) {
        // Strict sanitization: restrict to ASCII alphanumeric, hyphens, and underscores.
        // Explicitly reject any path separators, parent directory references, or excessive lengths.
        if !validate_locale(locale) {
            log::warn!(
                "I18nManager: BLOCKED attempt to load invalid locale string: '{}'",
                locale
            );
            return;
        }

        *self
            .locale
            .lock()
            .expect("I18nManager: locale lock poisoned") = locale.to_string();

        // Resolve path to bundled i18n JSON
        if let Ok(resource_dir) = app.path().resource_dir() {
            let i18n_dir = resource_dir.join("i18n").join(locale);

            // Try to find a JSON file in the locale directory
            if let Ok(entries) = fs::read_dir(i18n_dir) {
                for entry in entries.flatten() {
                    if entry.path().extension().and_then(|s| s.to_str()) == Some("json") {
                        if let Ok(content) = fs::read_to_string(entry.path()) {
                            if let Ok(json) =
                                serde_json::from_str::<HashMap<String, Value>>(&content)
                            {
                                let mut tr = self
                                    .translations
                                    .lock()
                                    .expect("I18nManager: translations lock poisoned");
                                tr.clear();
                                for (k, v) in json {
                                    if let Some(s) = v.as_str() {
                                        tr.insert(k, s.to_string());
                                    }
                                }
                                log::info!("Loaded locale: {}", locale);
                            }
                        }
                        break;
                    }
                }
            }
        }
    }

    pub fn get(&self, key: &str) -> String {
        let tr = self
            .translations
            .lock()
            .expect("I18nManager: translations lock poisoned");
        tr.get(key).cloned().unwrap_or_else(|| key.to_string())
    }
}

/// Validates that a locale string is safe and well-formed.
/// Restricts to ASCII alphanumeric, hyphens, and underscores, and limits length.
fn validate_locale(locale: &str) -> bool {
    if locale.is_empty() || locale.len() > 16 {
        return false;
    }
    locale
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

#[tauri::command]
pub fn get_translation(key: String, state: State<'_, I18nManager>) -> String {
    state.get(&key)
}

#[tauri::command]
pub fn set_locale(locale: String, app: AppHandle, state: State<'_, I18nManager>) {
    state.load_locale(&app, &locale);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_locale() {
        assert!(validate_locale("en-US"));
        assert!(validate_locale("pt_BR"));
        assert!(validate_locale("fr"));

        assert!(!validate_locale(""));
        assert!(!validate_locale("this-locale-is-too-long-for-sanitization"));
        assert!(!validate_locale("../../../tmp"));
        assert!(!validate_locale("en/US"));
        assert!(!validate_locale("en\\US"));
        assert!(!validate_locale("en;rm -rf /"));
    }
}
