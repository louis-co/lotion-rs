//! Spell checking.
//!
//! On Linux/Windows this is backed by Hunspell. On macOS WKWebView already
//! provides native, system-wide spell checking (NSSpellChecker), so the
//! commands are kept for API compatibility but defer to the platform.

use tauri::State;

#[cfg(not(target_os = "macos"))]
mod imp {
    use hunspell_rs::Hunspell;
    use std::sync::Mutex;

    pub struct SafeHunspell(pub Hunspell);
    unsafe impl Send for SafeHunspell {}
    unsafe impl Sync for SafeHunspell {}

    pub struct SpellcheckManager {
        pub hunspell: Mutex<Option<SafeHunspell>>,
    }

    impl SpellcheckManager {
        pub fn new() -> Self {
            let manager = Self {
                hunspell: Mutex::new(None),
            };

            // Cross-platform dictionary discovery
            let (aff, dic) = if cfg!(target_os = "windows") {
                (
                    "C:\\Program Files\\Hunspell\\en_US.aff".to_string(),
                    "C:\\Program Files\\Hunspell\\en_US.dic".to_string(),
                )
            } else {
                (
                    "/usr/share/hunspell/en_US.aff".to_string(),
                    "/usr/share/hunspell/en_US.dic".to_string(),
                )
            };

            if std::path::Path::new(&aff).exists() && std::path::Path::new(&dic).exists() {
                manager.load_dictionaries(&aff, &dic);
            } else {
                log::warn!(
                    "SpellcheckManager: Default en_US dictionaries not found at {}",
                    aff
                );
            }

            manager
        }

        pub fn load_dictionaries(&self, aff_path: &str, dic_path: &str) {
            let hs = Hunspell::new(aff_path, dic_path);
            *self
                .hunspell
                .lock()
                .expect("SpellcheckManager: hunspell lock poisoned") = Some(SafeHunspell(hs));
            log::info!("Hunspell dictionaries loaded successfully.");
        }

        pub fn check(&self, word: &str) -> bool {
            let hs_lock = self
                .hunspell
                .lock()
                .expect("SpellcheckManager: hunspell lock poisoned");
            if let Some(hs) = hs_lock.as_ref() {
                matches!(
                    hs.0.check(word),
                    hunspell_rs::CheckResult::FoundInDictionary
                )
            } else {
                true
            }
        }

        pub fn suggest(&self, word: &str) -> Vec<String> {
            let hs_lock = self
                .hunspell
                .lock()
                .expect("SpellcheckManager: hunspell lock poisoned");
            if let Some(hs) = hs_lock.as_ref() {
                hs.0.suggest(word)
            } else {
                Vec::new()
            }
        }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    pub struct SpellcheckManager;

    impl SpellcheckManager {
        pub fn new() -> Self {
            log::info!("SpellcheckManager: using native macOS spell checking (WKWebView).");
            Self
        }

        pub fn check(&self, _word: &str) -> bool {
            true
        }

        pub fn suggest(&self, _word: &str) -> Vec<String> {
            Vec::new()
        }
    }
}

pub use imp::SpellcheckManager;

impl Default for SpellcheckManager {
    fn default() -> Self {
        Self::new()
    }
}

#[tauri::command]
pub fn check_spelling(word: String, state: State<'_, SpellcheckManager>) -> bool {
    state.check(&word)
}

#[tauri::command]
pub fn get_spelling_suggestions(word: String, state: State<'_, SpellcheckManager>) -> Vec<String> {
    state.suggest(&word)
}
