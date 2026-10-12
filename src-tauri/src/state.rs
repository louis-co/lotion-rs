use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm,
    Key,
    Nonce, // Or `Nonce` from `aes_gcm::aead::AeadCore`
};
use base64::{engine::general_purpose, Engine as _};
use pbkdf2::pbkdf2_hmac;
use rand::rngs::StdRng;
use rand_core::{RngCore, SeedableRng};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::HashMap;
use std::path::PathBuf;

// const APPLICATION_SECRET: &[u8] = b"lotion-rs-super-secret-key-that-is-long-and-random-for-pbkdf2";
const PBKDF2_ITERATIONS: u32 = 100_000;
const NONCE_LEN: usize = 12;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedState {
    pub data: String,  // Base64 encoded encrypted data
    pub nonce: String, // Base64 encoded nonce
}

/// AES-256-GCM key protecting `state.json`.
pub type StateKey = Key<Aes256Gcm>;

/// Derive the AES-256-GCM key used for the state file. This is deliberately
/// expensive (PBKDF2); call it once and reuse the result.
pub fn derive_state_key(app_secret: &[u8]) -> Key<Aes256Gcm> {
    get_encryption_key(app_secret)
}

fn get_encryption_key(app_secret: &[u8]) -> Key<Aes256Gcm> {
    // Use a stable, but unique per-machine, salt for PBKDF2.
    // Combining the application name and config directory path provides this.
    let salt_source = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("lotion-rs")
        .to_string_lossy()
        .into_owned();

    let mut key_bytes = Key::<Aes256Gcm>::default();
    pbkdf2_hmac::<Sha256>(
        app_secret,
        salt_source.as_bytes(),
        PBKDF2_ITERATIONS,
        &mut key_bytes,
    );
    key_bytes
}

fn encrypt_data(data: &[u8], key: &Key<Aes256Gcm>) -> Result<(Vec<u8>, Vec<u8>), String> {
    let cipher = Aes256Gcm::new(key);
    let mut rng = StdRng::from_entropy();
    let mut nonce_bytes = vec![0u8; NONCE_LEN];
    rng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    cipher
        .encrypt(nonce, data)
        .map(|cipher_text| (cipher_text, nonce_bytes))
        .map_err(|e| format!("Encryption error: {:?}", e))
}

fn decrypt_data(
    encrypted_data: &[u8],
    nonce_bytes: &[u8],
    key: &Key<Aes256Gcm>,
) -> Result<Vec<u8>, String> {
    // GenericArray::from_slice panics on the wrong length. Saved state is
    // untrusted input: reject corruption so startup can fall back to fresh state.
    if nonce_bytes.len() != NONCE_LEN {
        return Err(format!(
            "Invalid state nonce length: expected {NONCE_LEN}, got {}",
            nonce_bytes.len()
        ));
    }
    let cipher = Aes256Gcm::new(key);
    let nonce = Nonce::from_slice(nonce_bytes);

    cipher
        .decrypt(nonce, encrypted_data)
        .map_err(|e| format!("Decryption error: {:?}", e))
}

/// Window geometry in logical (scale-independent) pixels.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bounds {
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TabState {
    pub id: String,
    pub title: String,
    pub url: String,
    pub is_active: bool,
    pub is_pinned: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowState {
    pub id: String,
    pub bounds: Bounds,
    pub is_focused: bool,
    pub is_maximized: bool,
    pub is_minimized: bool,
    pub is_full_screen: bool,
    pub tab_ids: Vec<String>,
    pub active_tab_id: Option<String>,
}

impl WindowState {
    pub fn new(id: &str) -> Self {
        Self {
            id: id.to_string(),
            bounds: Bounds {
                x: None,
                y: None,
                width: 1200.0,
                height: 800.0,
            },
            is_focused: true,
            is_maximized: false,
            is_minimized: false,
            is_full_screen: false,
            tab_ids: Vec::new(),
            active_tab_id: None,
        }
    }
}

/// Application state shared between Tauri callbacks. A std mutex: it is never
/// held across an await, and is used from both sync and async commands.
pub type SharedState = std::sync::Arc<std::sync::Mutex<AppState>>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppState {
    pub windows: HashMap<String, WindowState>,
    pub tabs: HashMap<String, TabState>,
    pub focused_window_id: Option<String>,
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

impl AppState {
    pub fn new() -> Self {
        Self {
            windows: HashMap::new(),
            tabs: HashMap::new(),
            focused_window_id: None,
        }
    }

    /// Returns the state file path (~/.config/lotion-rs/state.json)
    fn state_path() -> std::path::PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("lotion-rs")
            .join("state.json")
    }

    /// Save application state to disk.
    ///
    /// Derives the encryption key on every call (PBKDF2, 100k iterations), so
    /// hot paths should derive once with [`derive_state_key`] and use
    /// [`AppState::save_with_key`] instead (see `persistence::StateStore`).
    pub fn save_to_disk(&self, app_secret: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
        self.save_with_key(&get_encryption_key(app_secret))
    }

    /// Save application state to disk with a pre-derived key.
    ///
    /// The file is written to a temporary sibling and atomically renamed, so a
    /// crash mid-write can never leave a truncated (undecryptable) state file.
    pub fn save_with_key(&self, key: &Key<Aes256Gcm>) -> Result<(), Box<dyn std::error::Error>> {
        let path = Self::state_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let plaintext = serde_json::to_string(self)?;
        let (ciphertext, nonce) =
            encrypt_data(plaintext.as_bytes(), key).map_err(Box::<dyn std::error::Error>::from)?;

        let encrypted_state = EncryptedState {
            data: general_purpose::STANDARD.encode(&ciphertext),
            nonce: general_purpose::STANDARD.encode(&nonce),
        };
        let json = serde_json::to_string_pretty(&encrypted_state)?;

        let tmp_path = path.with_extension("json.tmp");
        std::fs::write(&tmp_path, json)?;
        #[cfg(target_family = "unix")]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::rename(&tmp_path, &path)?;
        log::debug!("Encrypted AppState saved to {}", path.display());
        Ok(())
    }

    /// Drop tab records that no window references any more.
    pub fn prune_orphan_tabs(&mut self) {
        let referenced: std::collections::HashSet<&String> = self
            .windows
            .values()
            .flat_map(|w| w.tab_ids.iter())
            .collect();
        self.tabs.retain(|id, _| referenced.contains(id));
    }

    /// Load application state from disk.
    pub fn load_from_disk(app_secret: &[u8]) -> Option<Self> {
        Self::load_with_key(&get_encryption_key(app_secret))
    }

    /// Load application state from disk with a pre-derived key.
    pub fn load_with_key(key: &Key<Aes256Gcm>) -> Option<Self> {
        let path = Self::state_path();
        Self::load_from_path(&path, key)
    }

    fn load_from_path(path: &std::path::Path, key: &Key<Aes256Gcm>) -> Option<Self> {
        if path.exists() {
            match std::fs::read_to_string(path) {
                Ok(contents) => {
                    // Try to load as encrypted state first
                    if let Ok(encrypted_state) = serde_json::from_str::<EncryptedState>(&contents) {
                        let decoded_data =
                            match general_purpose::STANDARD.decode(&encrypted_state.data) {
                                Ok(d) => d,
                                Err(e) => {
                                    log::warn!("Failed to base64 decode encrypted data: {}", e);
                                    return None;
                                }
                            };
                        let decoded_nonce =
                            match general_purpose::STANDARD.decode(&encrypted_state.nonce) {
                                Ok(n) => n,
                                Err(e) => {
                                    log::warn!("Failed to base64 decode nonce: {}", e);
                                    return None;
                                }
                            };

                        match decrypt_data(&decoded_data, &decoded_nonce, key) {
                            Ok(plaintext_bytes) => match String::from_utf8(plaintext_bytes) {
                                Ok(plaintext) => match serde_json::from_str::<AppState>(&plaintext)
                                {
                                    Ok(state) => {
                                        log::info!(
                                            "Encrypted AppState loaded from {}",
                                            path.display()
                                        );
                                        return Some(state);
                                    }
                                    Err(e) => log::warn!("Failed to parse decrypted state: {}", e),
                                },
                                Err(e) => {
                                    log::warn!("Failed to convert decrypted bytes to string: {}", e)
                                }
                            },
                            Err(e) => log::warn!("Failed to decrypt state file: {}", e),
                        }
                    }

                    // If encrypted load failed, try to load as plaintext (for backward compatibility)
                    // This branch will be executed if deserialization to EncryptedState fails,
                    // which means it's likely an old unencrypted file.
                    if let Ok(state) = serde_json::from_str::<AppState>(&contents) {
                        log::warn!(
                            "Loaded unencrypted AppState from {}. Please re-save to encrypt.",
                            path.display()
                        );
                        return Some(state);
                    }
                    log::warn!("Failed to load state file as either encrypted or plaintext.");
                }
                Err(e) => log::warn!("Failed to read state file: {}", e),
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_encrypted_fixture(path: &std::path::Path, data: &[u8], nonce: &[u8]) {
        let state = EncryptedState {
            data: general_purpose::STANDARD.encode(data),
            nonce: general_purpose::STANDARD.encode(nonce),
        };
        std::fs::write(path, serde_json::to_vec(&state).unwrap()).unwrap();
    }

    #[test]
    fn malformed_saved_nonces_are_rejected_without_panicking() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.json");
        let key = StateKey::from([42; 32]);
        for length in [0, 1, 11, 13, 64] {
            write_encrypted_fixture(&path, &[0; 16], &vec![0; length]);
            assert!(AppState::load_from_path(&path, &key).is_none());
        }
    }

    #[test]
    fn authenticated_encrypted_state_still_loads() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.json");
        let key = StateKey::from([42; 32]);
        let mut state = AppState::new();
        state
            .windows
            .insert("main".into(), WindowState::new("main"));
        state.focused_window_id = Some("main".into());
        let plaintext = serde_json::to_vec(&state).unwrap();
        let (ciphertext, nonce) = encrypt_data(&plaintext, &key).unwrap();
        write_encrypted_fixture(&path, &ciphertext, &nonce);
        let loaded = AppState::load_from_path(&path, &key).unwrap();
        assert_eq!(
            serde_json::to_value(loaded).unwrap(),
            serde_json::to_value(state).unwrap()
        );
    }

    #[test]
    fn invalid_ciphertext_with_valid_nonce_is_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.json");
        let key = StateKey::from([42; 32]);
        write_encrypted_fixture(&path, &[0; 16], &[0; NONCE_LEN]);
        assert!(AppState::load_from_path(&path, &key).is_none());
    }

    #[test]
    fn legacy_plaintext_state_still_loads() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.json");
        let state = AppState::new();
        std::fs::write(&path, serde_json::to_vec(&state).unwrap()).unwrap();
        assert!(AppState::load_from_path(&path, &StateKey::from([42; 32])).is_some());
    }
}
