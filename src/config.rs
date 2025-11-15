use serde::{Deserialize, Serialize};
use std::net::SocketAddr;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProxyMode {
    Forward,
    Reverse,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StaticMount {
    pub path: String,        // URL path prefix (e.g., "/app", "/api", "/assets")
    pub root_dir: String,     // Filesystem directory path
    #[serde(default)]
    pub enable_directory_listing: Option<bool>,
    #[serde(default)]
    pub index_files: Option<Vec<String>>,
    #[serde(default)]
    pub spa_mode: Option<bool>,
    #[serde(default)]
    pub spa_fallback_file: Option<String>,
}

impl StaticMount {
    pub fn resolve_inheritance(&self, parent_config: &StaticFileConfig) -> ResolvedStaticMount {
        ResolvedStaticMount {
            path: self.path.clone(),
            root_dir: self.root_dir.clone(),
            enable_directory_listing: self.enable_directory_listing
                .unwrap_or(parent_config.enable_directory_listing),
            index_files: self.index_files
                .clone()
                .unwrap_or_else(|| parent_config.index_files.clone()),
            spa_mode: self.spa_mode
                .unwrap_or(parent_config.spa_mode),
            spa_fallback_file: self.spa_fallback_file
                .clone()
                .unwrap_or_else(|| parent_config.spa_fallback_file.clone()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedStaticMount {
    pub path: String,
    pub root_dir: String,
    pub enable_directory_listing: bool,
    pub index_files: Vec<String>,
    pub spa_mode: bool,
    pub spa_fallback_file: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StaticFileConfig {
    pub mounts: Vec<StaticMount>,
    pub enable_directory_listing: bool,
    pub index_files: Vec<String>,
    pub spa_mode: bool,
    pub spa_fallback_file: String,
    pub worker_threads: Option<usize>,
    #[serde(default)]
    pub custom_mime_types: std::collections::HashMap<String, String>,
}

// For backward compatibility
impl Default for StaticFileConfig {
    fn default() -> Self {
        Self {
            mounts: vec![StaticMount {
                path: "/".to_string(),
                root_dir: "./public".to_string(),
                enable_directory_listing: None, // Will inherit from parent
                index_files: None, // Will inherit from parent
                spa_mode: None, // Will inherit from parent
                spa_fallback_file: None, // Will inherit from parent
            }],
            enable_directory_listing: false,
            index_files: vec!["index.html".to_string(), "index.htm".to_string()],
            spa_mode: false,
            spa_fallback_file: "index.html".to_string(),
            worker_threads: None,
            custom_mime_types: std::collections::HashMap::new(),
        }
    }
}

impl StaticFileConfig {
    pub fn single(root_dir: String, spa_mode: bool) -> Self {
        Self {
            mounts: vec![StaticMount {
                path: "/".to_string(),
                root_dir,
                enable_directory_listing: None, // Will inherit from parent
                index_files: None, // Will inherit from parent
                spa_mode: Some(spa_mode), // Override SPA mode
                spa_fallback_file: None, // Will inherit from parent
            }],
            enable_directory_listing: false,
            index_files: vec!["index.html".to_string(), "index.htm".to_string()],
            spa_mode,
            spa_fallback_file: "index.html".to_string(),
            worker_threads: None,
            custom_mime_types: std::collections::HashMap::new(),
        }
    }

    pub fn add_mount(&mut self, path: String, root_dir: String, spa_mode: bool) {
        self.mounts.push(StaticMount {
            path,
            root_dir,
            enable_directory_listing: None, // Will inherit from parent
            index_files: None, // Will inherit from parent
            spa_mode: Some(spa_mode), // Override SPA mode
            spa_fallback_file: None, // Will inherit from parent
        });
    }

    pub fn add_custom_mime_type(&mut self, extension: String, mime_type: String) {
        // Remove leading dot if present
        let clean_ext = extension.strip_prefix('.').unwrap_or(&extension).to_lowercase();
        self.custom_mime_types.insert(clean_ext, mime_type);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub mode: ProxyMode,
    pub listen_addr: SocketAddr,
    pub reverse_proxy_target: Option<String>,
    pub forward_proxy_port: Option<u16>,
    pub max_connections: Option<usize>,
    // New timeout configurations
    #[serde(default)]
    pub connect_timeout_secs: Option<u64>,
    #[serde(default)]
    pub idle_timeout_secs: Option<u64>,
    #[serde(default)]
    pub max_connection_lifetime_secs: Option<u64>,
    // Legacy timeout field for backward compatibility
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    pub static_files: Option<StaticFileConfig>,
    #[serde(default)]
    pub private_key: Option<String>,
    #[serde(default)]
    pub certificate: Option<String>,
    #[serde(default)]
    pub connection_pool_enabled: Option<bool>,
    #[serde(default)]
    pub pool_max_idle_per_host: Option<usize>,
    #[serde(default = "default_max_header_size")]
    pub max_header_size: Option<usize>,
}

fn default_max_header_size() -> Option<usize> {
    Some(16 * 1024) // 16KB default header size limit
}

impl Default for Config {
    fn default() -> Self {
        Self {
            mode: ProxyMode::Forward,
            listen_addr: "127.0.0.1:8080".parse().unwrap(),
            reverse_proxy_target: None,
            forward_proxy_port: Some(3128),
            max_connections: Some(1000),
            connect_timeout_secs: Some(10),
            idle_timeout_secs: Some(90),
            max_connection_lifetime_secs: Some(300),
            timeout_secs: None,
            static_files: None,
            private_key: None,
            certificate: None,
            connection_pool_enabled: Some(true),
            pool_max_idle_per_host: Some(10),
            max_header_size: default_max_header_size(),
        }
    }
}

impl Config {
    pub fn from_file(path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let content = std::fs::read_to_string(path)?;
        let config: Config = serde_json::from_str(&content)?;
        Ok(config)
    }

    pub fn to_file(&self, path: &str) -> Result<(), Box<dyn std::error::Error>> {
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }
}