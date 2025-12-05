use crate::config::Config;
use aes_gcm::{aead::{Aead, KeyInit}, Aes256Gcm, Nonce};
use base64::{engine::general_purpose, Engine as _};
use log::{info, warn};
use prometheus::{IntCounter, Opts, Registry};
use rand::rngs::OsRng;
use rand::RngCore;
use std::fs;
use std::path::PathBuf;
#[cfg(unix)]
use std::path::Path;
use std::sync::{atomic::{AtomicBool, Ordering}, OnceLock};
use thiserror::Error;
use zeroize::Zeroizing;

const KEY_SIZE: usize = 32;
const NONCE_SIZE: usize = 12;
const ENCRYPTED_PREFIX: &str = "{encrypted}";
const MASK_FILE: &str = "master_key.mask";
const PART_FILES: [&str; 3] = ["master_key.part1", "master_key.part2", "master_key.part3"];
const ENV_OVERRIDE: &str = "BIFROST_SECRET_HOME";

/// Errors produced by the secret management workflow.
#[derive(Error, Debug)]
pub enum SecretError {
    #[error("unable to locate a writable home directory for secrets")]
    MissingHomeDir,

    #[error("secret storage directory already contains a master key")]
    KeyAlreadyInitialized,

    #[error("encryption key has not been initialized yet")]
    KeyNotInitialized,

    #[error("secret storage directory permissions are insecure (expected 0700)")]
    InsecurePermissions,

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("base64 decoding error: {0}")]
    Base64(#[from] base64::DecodeError),

    #[error("encryption error: {0}")]
    Encryption(String),

    #[error("invalid encrypted payload: {0}")]
    InvalidPayload(String),

    #[error("decrypted secret is not valid UTF-8")]
    InvalidUtf8(#[from] std::string::FromUtf8Error),
}

struct SecretTelemetry {
    decrypt_success: IntCounter,
    decrypt_failure: IntCounter,
    registered: AtomicBool,
}

impl SecretTelemetry {
    fn new() -> Self {
        let success_opts = Opts::new(
            "config_secret_decrypt_success_total",
            "Number of encrypted configuration values successfully decrypted",
        ).namespace("bifrost");
        let failure_opts = Opts::new(
            "config_secret_decrypt_failure_total",
            "Number of encrypted configuration values that failed to decrypt",
        ).namespace("bifrost");
        Self {
            decrypt_success: IntCounter::with_opts(success_opts)
                .expect("decrypt_success counter"),
            decrypt_failure: IntCounter::with_opts(failure_opts)
                .expect("decrypt_failure counter"),
            registered: AtomicBool::new(false),
        }
    }

    fn register_if_needed(&self, registry: &Registry) {
        if self.registered.load(Ordering::Relaxed) {
            return;
        }
        if let Err(err) = registry.register(Box::new(self.decrypt_success.clone())) {
            warn!("Failed to register decrypt_success metric: {}", err);
            return;
        }
        if let Err(err) = registry.register(Box::new(self.decrypt_failure.clone())) {
            warn!("Failed to register decrypt_failure metric: {}", err);
            return;
        }
        self.registered.store(true, Ordering::Relaxed);
    }

    fn inc_success(&self) {
        self.decrypt_success.inc();
    }

    fn inc_failure(&self) {
        self.decrypt_failure.inc();
    }
}

fn telemetry() -> &'static SecretTelemetry {
    static TELEMETRY: OnceLock<SecretTelemetry> = OnceLock::new();
    TELEMETRY.get_or_init(SecretTelemetry::new)
}

pub fn register_secret_metrics(registry: &Registry) {
    telemetry().register_if_needed(registry);
}

/// Helper responsible for encrypting, decrypting, and persisting secrets.
pub struct SecretManager {
    root_dir: PathBuf,
}

impl SecretManager {
    /// Creates a new manager targeting `~/.bifrost` or a directory override.
    pub fn new() -> Result<Self, SecretError> {
        let root_dir = resolve_secret_home()?;
        Ok(Self { root_dir })
    }

    #[cfg(test)]
    fn with_root(root_dir: PathBuf) -> Result<Self, SecretError> {
        Ok(Self { root_dir })
    }

    /// Initializes the AES-256 key and persists masked fragments on disk.
    pub fn init_encryption_key(&self, overwrite: bool) -> Result<(), SecretError> {
        self.ensure_root_dir()?;

        if !overwrite && self.key_material_exists() {
            return Err(SecretError::KeyAlreadyInitialized);
        }

        if overwrite {
            self.remove_existing_material()?;
        }

        let mut key = Zeroizing::new([0u8; KEY_SIZE]);
        OsRng.fill_bytes(&mut key[..]);

        let mut mask = Zeroizing::new([0u8; KEY_SIZE]);
        OsRng.fill_bytes(&mut mask[..]);

        self.persist_key_fragments(&key, &mask)?;

        info!(
            "Initialized encryption key in {}",
            self.root_dir.display()
        );
        Ok(())
    }

    /// Encrypts a payload and returns the canonical `{encrypted}<base64>` token.
    pub fn encrypt_payload(&self, payload: &[u8]) -> Result<String, SecretError> {
        self.ensure_key_available()?;
        let key = self.recover_key()?;
        let cipher = Aes256Gcm::new_from_slice(&key[..])
            .map_err(|e| SecretError::Encryption(e.to_string()))?;
        let mut nonce = [0u8; NONCE_SIZE];
        OsRng.fill_bytes(&mut nonce);

        let ciphertext = cipher
            .encrypt(Nonce::from_slice(&nonce), payload)
            .map_err(|e| SecretError::Encryption(e.to_string()))?;

        let mut bundle = Vec::with_capacity(NONCE_SIZE + ciphertext.len());
        bundle.extend_from_slice(&nonce);
        bundle.extend_from_slice(&ciphertext);

        let token = format!(
            "{}{}",
            ENCRYPTED_PREFIX,
            general_purpose::STANDARD.encode(bundle)
        );
        info!("Produced encrypted secret payload ({} bytes)", payload.len());
        Ok(token)
    }

    /// Attempts to decrypt a `{encrypted}` payload and returns the plaintext string.
    pub fn decrypt_secret_string(&self, value: &str) -> Result<String, SecretError> {
        let plaintext = self.decrypt_secret_bytes(value)?;
        let secret = String::from_utf8(plaintext)?;
        Ok(secret)
    }

    /// Detects the prefix and decrypts when necessary. Returns `true` if decrypted.
    pub fn decrypt_option_field(
        &self,
        field: &mut Option<String>,
        field_name: &str,
    ) -> Result<bool, SecretError> {
        if let Some(current) = field.as_mut() {
            if let Some(stripped) = current.strip_prefix(ENCRYPTED_PREFIX) {
                match self.decrypt_secret_string(stripped) {
                    Ok(secret) => {
                        *current = secret;
                        telemetry().inc_success();
                        info!("Decrypted encrypted secret for {}", field_name);
                        return Ok(true);
                    }
                    Err(err) => {
                        telemetry().inc_failure();
                        return Err(err);
                    }
                }
            }
        }
        Ok(false)
    }

    pub fn encrypted_prefix() -> &'static str {
        ENCRYPTED_PREFIX
    }

    pub fn apply_to_config(&self, config: &mut Config) -> Result<(), SecretError> {
        self.apply_to_top_level(config)?;
        self.apply_to_relays(config)?;
        Ok(())
    }

    fn apply_to_top_level(&self, config: &mut Config) -> Result<(), SecretError> {
        self.decrypt_option_field(&mut config.proxy_password, "config.proxy_password")?;
        self.decrypt_option_field(
            &mut config.relay_proxy_password,
            "config.relay_proxy_password",
        )?;
        Ok(())
    }

    fn apply_to_relays(&self, config: &mut Config) -> Result<(), SecretError> {
        if let Some(relays) = config.relay_proxies.as_mut() {
            for (idx, relay) in relays.iter_mut().enumerate() {
                self.decrypt_option_field(
                    &mut relay.relay_proxy_password,
                    &format!("config.relay_proxies[{}].relay_proxy_password", idx),
                )?;
            }
        }
        Ok(())
    }

    fn decrypt_secret_bytes(&self, value: &str) -> Result<Vec<u8>, SecretError> {
        self.ensure_key_available()?;
        let payload = if let Some(stripped) = value.strip_prefix(ENCRYPTED_PREFIX) {
            stripped
        } else {
            value
        };
        let bundle = general_purpose::STANDARD
            .decode(payload.trim())
            .map_err(SecretError::Base64)?;
        if bundle.len() <= NONCE_SIZE {
            return Err(SecretError::InvalidPayload(
                "payload too small to contain nonce and ciphertext".to_string(),
            ));
        }
        let (nonce_bytes, ciphertext) = bundle.split_at(NONCE_SIZE);
        let key = self.recover_key()?;
        let cipher = Aes256Gcm::new_from_slice(&key[..])
            .map_err(|e| SecretError::Encryption(e.to_string()))?;
        cipher
            .decrypt(Nonce::from_slice(nonce_bytes), ciphertext)
            .map_err(|e| SecretError::Encryption(e.to_string()))
    }

    fn persist_key_fragments(
        &self,
        key: &Zeroizing<[u8; KEY_SIZE]>,
        mask: &Zeroizing<[u8; KEY_SIZE]>,
    ) -> Result<(), SecretError> {
        #[cfg(unix)]
        self.enforce_permissions(&self.root_dir)?;

        fs::write(
            self.root_dir.join(MASK_FILE),
            general_purpose::STANDARD.encode(&mask[..]),
        )?;

        let splits = [(0, 11), (11, 22), (22, KEY_SIZE)];
        for (idx, (start, end)) in splits.into_iter().enumerate() {
            let mut fragment = Vec::with_capacity(end - start);
            for offset in start..end {
                fragment.push(key[offset] ^ mask[offset]);
            }
            let encoded = general_purpose::STANDARD.encode(fragment);
            fs::write(self.root_dir.join(PART_FILES[idx]), encoded)?;
        }
        Ok(())
    }

    fn ensure_root_dir(&self) -> Result<(), SecretError> {
        if !self.root_dir.exists() {
            fs::create_dir_all(&self.root_dir)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let perms = fs::Permissions::from_mode(0o700);
                fs::set_permissions(&self.root_dir, perms)?;
            }
        }
        #[cfg(unix)]
        self.enforce_permissions(&self.root_dir)?;
        Ok(())
    }

    #[cfg(unix)]
    fn enforce_permissions(&self, path: &Path) -> Result<(), SecretError> {
        use std::os::unix::fs::PermissionsExt;
        let metadata = fs::metadata(path)?;
        let mode = metadata.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            return Err(SecretError::InsecurePermissions);
        }
        Ok(())
    }

    fn key_material_exists(&self) -> bool {
        self.root_dir.join(MASK_FILE).exists()
            && PART_FILES
                .iter()
                .all(|file| self.root_dir.join(file).exists())
    }

    fn remove_existing_material(&self) -> Result<(), SecretError> {
        if self.root_dir.join(MASK_FILE).exists() {
            fs::remove_file(self.root_dir.join(MASK_FILE))?;
        }
        for file in PART_FILES {
            let path = self.root_dir.join(file);
            if path.exists() {
                fs::remove_file(path)?;
            }
        }
        Ok(())
    }

    fn ensure_key_available(&self) -> Result<(), SecretError> {
        if !self.key_material_exists() {
            return Err(SecretError::KeyNotInitialized);
        }
        #[cfg(unix)]
        self.enforce_permissions(&self.root_dir)?;
        Ok(())
    }

    fn recover_key(&self) -> Result<Zeroizing<[u8; KEY_SIZE]>, SecretError> {
        let mask_encoded = fs::read_to_string(self.root_dir.join(MASK_FILE))?;
        let mask_bytes = general_purpose::STANDARD.decode(mask_encoded.trim())?;
        if mask_bytes.len() != KEY_SIZE {
            return Err(SecretError::InvalidPayload(
                "mask size mismatch".to_string(),
            ));
        }

        let mut mask_array = Zeroizing::new([0u8; KEY_SIZE]);
        mask_array.copy_from_slice(&mask_bytes);

        let mut key = Zeroizing::new([0u8; KEY_SIZE]);
        let splits = [(0, 11), (11, 22), (22, KEY_SIZE)];

        for (idx, (start, end)) in splits.into_iter().enumerate() {
            let encoded = fs::read_to_string(self.root_dir.join(PART_FILES[idx]))?;
            let decoded = general_purpose::STANDARD.decode(encoded.trim())?;
            if decoded.len() != end - start {
                return Err(SecretError::InvalidPayload(format!(
                    "fragment {} has invalid length",
                    idx + 1
                )));
            }
            for (offset, value) in decoded.iter().enumerate() {
                key[start + offset] = value ^ mask_array[start + offset];
            }
        }

        Ok(key)
    }
}

fn resolve_secret_home() -> Result<PathBuf, SecretError> {
    if let Ok(dir) = std::env::var(ENV_OVERRIDE) {
        return Ok(PathBuf::from(dir));
    }
    let home = dirs::home_dir().ok_or(SecretError::MissingHomeDir)?;
    Ok(home.join(".bifrost"))
}

fn option_needs_decrypt(value: &Option<String>) -> bool {
    value
        .as_ref()
        .map(|v| v.starts_with(ENCRYPTED_PREFIX))
        .unwrap_or(false)
}

pub fn config_has_encrypted_values(config: &Config) -> bool {
    if option_needs_decrypt(&config.proxy_password)
        || option_needs_decrypt(&config.relay_proxy_password)
    {
        return true;
    }
    if let Some(relays) = &config.relay_proxies {
        if relays
            .iter()
            .any(|relay| option_needs_decrypt(&relay.relay_proxy_password))
        {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn init_and_round_trip_encryption() {
        let dir = tempdir().unwrap();
        let manager = SecretManager::with_root(dir.path().to_path_buf()).unwrap();
        manager.init_encryption_key(false).unwrap();

        let token = manager.encrypt_payload(b"relay-secret").unwrap();
        assert!(token.starts_with(ENCRYPTED_PREFIX));

        let decrypted = manager.decrypt_secret_string(&token).unwrap();
        assert_eq!(decrypted, "relay-secret");
    }

    #[test]
    fn decrypt_option_field_updates_value() {
        let dir = tempdir().unwrap();
        let manager = SecretManager::with_root(dir.path().to_path_buf()).unwrap();
        manager.init_encryption_key(false).unwrap();

        let token = manager.encrypt_payload(b"top-secret").unwrap();
        let mut value = Some(token);
        let changed = manager
            .decrypt_option_field(&mut value, "test.field")
            .unwrap();
        assert!(changed);
        assert_eq!(value.unwrap(), "top-secret");
    }
}
