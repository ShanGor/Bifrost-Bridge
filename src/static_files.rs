use crate::error::ProxyError;
use crate::config::{StaticFileConfig, ResolvedStaticMount};
use crate::common::{FileStreaming, FileBody};
use hyper::{Method, Response, StatusCode};
use hyper::body::Incoming;
use http_body_util::Full;
use hyper::body::Bytes;
use std::fs;
use std::path::{Path, PathBuf};

// HTML Templates - extracted as constants for maintainability and performance

/// Template for 404 Not Found error page
const HTML_404_TEMPLATE: &str = r#"<!DOCTYPE html>
<html>
<head><title>404 Not Found</title></head>
<body>
    <h1>404 Not Found</h1>
    <p>The requested resource was not found on this server.</p>
</body>
</html>"#;

/// Template for directory listing page header
const HTML_DIR_LISTING_HEADER: &str = r#"<!DOCTYPE html>
<html>
<head>
    <title>Directory listing for {path}</title>
    <style>
        body {{ font-family: Arial, sans-serif; margin: 40px; }}
        h1 {{ color: #333; }}
        ul {{ list-style: none; padding: 0; }}
        li {{ padding: 8px 0; }}
        a {{ text-decoration: none; color: #0066cc; }}
        a:hover {{ text-decoration: underline; }}
        .directory {{ font-weight: bold; }}
    </style>
</head>
<body>
    <h1>Directory listing for {path}</h1>
    <ul>"#;

/// Template for directory listing page footer
const HTML_DIR_LISTING_FOOTER: &str = r#"    </ul>
</body>
</html>"#;

/// Template for parent directory link in directory listing
const HTML_DIR_PARENT_LINK: &str = r#"        <li><a href="../">📁 ../</a></li>"#;

/// Template for directory entry in directory listing
const HTML_DIR_ENTRY_TEMPLATE: &str = r#"        <li class="{class}"><a href="{href}">{icon}</a></li>"#;

/// Helper function to detect if a file is an index file based on configuration
fn is_index_file(path: &Path, index_files: &[String]) -> bool {
    if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
        index_files.iter().any(|index| index.to_lowercase() == file_name.to_lowercase())
    } else {
        false
    }
}

/// Helper function to detect if a file matches no-cache patterns
fn is_no_cache_file(path: &Path, no_cache_files: &[String]) -> bool {
    if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
        no_cache_files.iter().any(|pattern| {
            // Support both exact matches and extension patterns like "*.js"
            if pattern.starts_with("*.") {
                // Extension pattern (e.g., "*.js" matches any .js file)
                let ext = pattern.strip_prefix("*.").unwrap().to_lowercase();
                if let Some(file_ext) = std::path::Path::new(file_name).extension().and_then(|e| e.to_str()) {
                    return file_ext.to_lowercase() == ext;
                }
            } else {
                // Exact filename match (case-insensitive)
                return pattern.to_lowercase() == file_name.to_lowercase();
            }
            false
        })
    } else {
        false
    }
}

/// Helper function to determine if no-cache headers should be used for SPA files and custom no-cache patterns
fn should_use_no_cache_for_spa(file_path: &Path, spa_mode: bool, is_spa_fallback: bool, index_files: &[String], no_cache_files: &[String]) -> bool {
    is_spa_fallback ||
    (spa_mode && is_index_file(file_path, index_files)) ||
    is_no_cache_file(file_path, no_cache_files)
}

#[derive(Clone)]
pub struct StaticFileHandler {
    // Pre-computed mount information for faster lookup
    mounts: Vec<MountInfo>,
    // Custom MIME type mappings
    custom_mime_types: std::collections::HashMap<String, String>,
}

#[derive(Clone)]
pub struct MountInfo {
    resolved_mount: ResolvedStaticMount,
    root_path: std::path::PathBuf,
    path_len: usize,
}

impl StaticFileHandler {
    pub fn new(config: StaticFileConfig) -> Result<Self, ProxyError> {
        let mut mounts = Vec::new();

        for mount in &config.mounts {
            let resolved_mount = mount.resolve_inheritance(&config);
            let root_path = Path::new(&resolved_mount.root_dir).canonicalize()
                .map_err(|e| ProxyError::Config(format!("Invalid root directory '{}': {}", resolved_mount.root_dir, e)))?;

            mounts.push(MountInfo {
                resolved_mount,
                root_path,
                path_len: mount.path.len(),
            });
        }

        // Sort mounts by path length (longest first) to ensure proper matching
        mounts.sort_by(|a, b| b.path_len.cmp(&a.path_len));

        Ok(Self {
            mounts,
            custom_mime_types: config.custom_mime_types,
        })
    }

    pub async fn handle_request(&self, req: &hyper::Request<Incoming>) -> Result<Response<FileBody>, ProxyError> {
        if req.method() != &Method::GET && req.method() != &Method::HEAD {
            return Ok(Response::builder()
                .status(StatusCode::METHOD_NOT_ALLOWED)
                .header("Allow", "GET, HEAD")
                .body(FileBody::InMemory(Full::new(Bytes::new())))
                .map_err(|e| ProxyError::Http(e.to_string()))?);
        }

        let path = req.uri().path();

        // Find the best matching mount for this path
        let (mount_info, relative_path) = match self.find_mount_for_path(path) {
            Some(result) => result,
            None => return Ok(self.not_found_response()),
        };

        // Resolve the file path within the mount
        let file_path = self.resolve_file_path_in_mount(&mount_info, &relative_path)?;

        if !file_path.exists() {
            // If SPA mode is enabled for this mount, check if this should use fallback or return 404
            if mount_info.resolved_mount.spa_mode {
                // Don't use SPA fallback for asset files - they should return 404 if missing
                if !self.is_asset_file(&relative_path) {
                    return self.handle_spa_fallback_in_mount(&mount_info, req.method() == &Method::HEAD).await;
                }
            }
            return Ok(self.not_found_response());
        }

        if file_path.is_dir() {
            return self.handle_directory_in_mount(mount_info, &file_path, &relative_path, req.method() == Method::HEAD).await;
        }

        self.handle_file_with_mount_info(&file_path, req.method() == Method::HEAD, Some(mount_info), false).await
    }

    pub fn find_mount_for_path(&self, path: &str) -> Option<(&MountInfo, String)> {
        for mount_info in &self.mounts {
            if path.starts_with(&mount_info.resolved_mount.path) {
                let relative_path = if mount_info.resolved_mount.path == "/" {
                    path.to_string()
                } else {
                    path[mount_info.resolved_mount.path.len()..].to_string()
                };
                return Some((mount_info, relative_path));
            }
        }
        None
    }

    fn resolve_file_path_in_mount(&self, mount_info: &MountInfo, relative_path: &str) -> Result<PathBuf, ProxyError> {
        let clean_path = if relative_path.is_empty() || relative_path == "/" {
            "/"
        } else {
            relative_path
        };

        let requested_path = match clean_path {
            "/" => mount_info.root_path.clone(),
            _ => {
                // Remove leading slash if present
                let path_without_leading = clean_path.strip_prefix('/').unwrap_or(clean_path);
                mount_info.root_path.join(path_without_leading)
            }
        };

        Ok(requested_path)
    }

    async fn handle_spa_fallback_in_mount(&self, mount_info: &MountInfo, is_head: bool) -> Result<Response<FileBody>, ProxyError> {
        let fallback_path = mount_info.root_path.join(&mount_info.resolved_mount.spa_fallback_file);

        // Check if fallback file exists
        if !fallback_path.exists() || !fallback_path.is_file() {
            return Ok(self.not_found_response());
        }

        self.handle_file_with_mount_info(&fallback_path, is_head, Some(mount_info), true).await
    }

    async fn handle_directory_in_mount(&self, mount_info: &MountInfo, dir_path: &PathBuf, request_path: &str, is_head: bool) -> Result<Response<FileBody>, ProxyError> {
        if !mount_info.resolved_mount.enable_directory_listing {
            // Try to serve index files for directories
            for index_file in &mount_info.resolved_mount.index_files {
                let index_path = dir_path.join(index_file);
                if index_path.exists() && index_path.is_file() {
                    return self.handle_file_with_mount_info(&index_path, is_head, Some(mount_info), false).await;
                }
            }

            // If SPA mode is enabled, try fallback
            if mount_info.resolved_mount.spa_mode {
                return self.handle_spa_fallback_in_mount(mount_info, is_head).await;
            }

            return Ok(self.not_found_response());
        }

        self.generate_directory_listing_in_mount(dir_path, request_path, is_head).await
    }

    async fn generate_directory_listing_in_mount(&self, dir_path: &Path, request_path: &str, is_head: bool) -> Result<Response<FileBody>, ProxyError> {
        let dir_path_clone = dir_path.to_path_buf();
        let request_path_clone = request_path.to_string();

        // Use tokio::spawn_blocking for CPU-intensive directory operations
        let html = tokio::task::spawn_blocking(move || {
            let entries = match fs::read_dir(&dir_path_clone) {
                Ok(entries) => entries,
                Err(_) => return String::new(), // Will trigger not_found_response
            };

            // Start with header template
            let mut html = HTML_DIR_LISTING_HEADER
                .replace("{path}", &request_path_clone);

            // Add parent directory link if not at root
            if request_path_clone != "/" {
                html.push_str(HTML_DIR_PARENT_LINK);
                html.push('\n');
            }

            // List entries
            for entry in entries {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(_) => continue, // Skip problematic entries
                };
                let file_name = entry.file_name();
                let file_name_str = file_name.to_string_lossy();

                let file_type = match entry.file_type() {
                    Ok(file_type) => file_type,
                    Err(_) => continue, // Skip problematic entries
                };
                let is_dir = file_type.is_dir();

                let icon = if is_dir { "📁" } else { "📄" };
                let class = if is_dir { "directory" } else { "file" };
                let href = format!(
                    "{}{}",
                    file_name_str,
                    if is_dir { "/" } else { "" }
                );

                let entry_html = HTML_DIR_ENTRY_TEMPLATE
                    .replace("{class}", class)
                    .replace("{href}", &href)
                    .replace("{icon}", icon);
                
                html.push_str(&entry_html);
                html.push('\n');
            }

            // Add footer
            html.push_str(HTML_DIR_LISTING_FOOTER);

            html
        }).await;

        let html = html.map_err(|e| ProxyError::Config(format!("Directory listing error: {}", e)))?;

        if html.is_empty() {
            return Ok(self.not_found_response());
        }

        let content_length = html.len();
        let body = if is_head {
            FileBody::InMemory(Full::new(Bytes::new()))
        } else {
            FileBody::InMemory(Full::new(Bytes::from(html)))
        };

        Ok(Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "text/html; charset=utf-8")
            .header("Content-Length", content_length.to_string())
            .body(body)
            .map_err(|e| ProxyError::Http(e.to_string()))?)
    }

    
    // resolve_file_path is replaced by resolve_file_path_in_mount for multi-mount support

    /// Handle file with optional mount information for SPA-aware caching
    pub async fn handle_file_with_mount_info(
        &self,
        file_path: &PathBuf,
        is_head: bool,
        mount_info: Option<&MountInfo>,
        is_spa_fallback: bool,
    ) -> Result<Response<FileBody>, ProxyError> {
        let metadata = fs::metadata(file_path)
            .map_err(|_| ProxyError::NotFound(format!("File not found: {:?}", file_path)))?;

        if !metadata.is_file() {
            return Ok(self.not_found_response());
        }

        // Use tokio::spawn_blocking for CPU-intensive MIME type detection
        let file_path_clone = file_path.clone();
        let custom_mime_types_clone = self.custom_mime_types.clone();
        let mime_type = tokio::task::spawn_blocking(move || {
            Self::guess_mime_type_static(&file_path_clone, &custom_mime_types_clone)
        }).await.map_err(|e| ProxyError::Config(format!("MIME type detection error: {}", e)))?;

        let _last_modified = metadata.modified()
            .map_err(|e| ProxyError::Config(format!("Cannot get file metadata: {}", e)))?;

        // Check file size and use optimized serving strategy
        let file_size = FileStreaming::get_file_size(file_path).await?;

        // Determine if we should use no-cache headers
        let spa_mode = mount_info.map(|m| m.resolved_mount.spa_mode).unwrap_or(false);
        let no_cache = if let Some(mount_info) = mount_info {
            should_use_no_cache_for_spa(
                file_path,
                spa_mode,
                is_spa_fallback,
                &mount_info.resolved_mount.index_files,
                &mount_info.resolved_mount.no_cache_files
            )
        } else {
            should_use_no_cache_for_spa(file_path, spa_mode, is_spa_fallback, &vec![], &vec![])
        };

        // Get cache duration from configuration
        let cache_duration = mount_info
            .map(|m| m.resolved_mount.cache_millisecs)
            .unwrap_or(3600);

        // Use centralized optimized response with SPA-aware cache control and streaming support
        FileStreaming::create_optimized_file_response(file_path, &mime_type, file_size, is_head, no_cache, cache_duration).await
    }

    // handle_directory is replaced by handle_directory_in_mount for multi-mount support

    // handle_spa_fallback is replaced by handle_spa_fallback_in_mount for multi-mount support

    
    /// Generates a 404 Not Found response
    fn not_found_response(&self) -> Response<FileBody> {
        Response::builder()
            .status(StatusCode::NOT_FOUND)
            .header("Content-Type", "text/html; charset=utf-8")
            .body(FileBody::InMemory(Full::new(Bytes::from(HTML_404_TEMPLATE))))
            .unwrap()
    }

    fn is_asset_file(&self, path: &str) -> bool {
        // Check if the path has an asset file extension
        if let Some(extension) = Path::new(path).extension().and_then(|ext| ext.to_str()) {
            matches!(extension.to_lowercase().as_str(), 
                "js" | "css" | "png" | "jpg" | "jpeg" | "gif" | "svg" | "ico" |
                "woff" | "woff2" | "ttf" | "eot" | "pdf" | "zip" | "json" | "xml" |
                "mp4" | "webm" | "mp3" | "wav")
        } else {
            false
        }
    }

    fn guess_mime_type_static(file_path: &PathBuf, custom_mime_types: &std::collections::HashMap<String, String>) -> String {
        if let Some(extension) = file_path.extension().and_then(|ext| ext.to_str()) {
            let ext_lower = extension.to_lowercase();

            // Check custom MIME types first - allows overriding mime_guess
            if let Some(custom_mime) = custom_mime_types.get(&ext_lower) {
                return custom_mime.clone();
            }
        }

        // Use mime_guess for comprehensive MIME type detection
        let mime = mime_guess::from_path(file_path)
            .first_or_octet_stream();

        // Add charset for text-based MIME types
        let mime_str = mime.as_ref();
        if mime_str.starts_with("text/") || 
           mime_str == "application/json" ||
           mime_str == "application/xml" {
            format!("{}; charset=utf-8", mime_str)
        } else {
            mime_str.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::StaticFileConfig;

    #[test]
    fn test_mime_type_detection() {
        // Test static method directly
        let custom_mime_types = std::collections::HashMap::new();
        assert_eq!(StaticFileHandler::guess_mime_type_static(&PathBuf::from("test.html"), &custom_mime_types), "text/html; charset=utf-8");
        assert_eq!(StaticFileHandler::guess_mime_type_static(&PathBuf::from("test.css"), &custom_mime_types), "text/css; charset=utf-8");
        // mime_guess returns "text/javascript" which is the modern standard (RFC 9239)
        assert_eq!(StaticFileHandler::guess_mime_type_static(&PathBuf::from("test.js"), &custom_mime_types), "text/javascript; charset=utf-8");
        assert_eq!(StaticFileHandler::guess_mime_type_static(&PathBuf::from("test.png"), &custom_mime_types), "image/png");
        assert_eq!(StaticFileHandler::guess_mime_type_static(&PathBuf::from("test.unknown"), &custom_mime_types), "application/octet-stream");
        
        // Test custom MIME type override
        let mut custom_mime_types = std::collections::HashMap::new();
        custom_mime_types.insert("custom".to_string(), "application/x-custom".to_string());
        assert_eq!(StaticFileHandler::guess_mime_type_static(&PathBuf::from("test.custom"), &custom_mime_types), "application/x-custom");
    }

    #[test]
    fn test_path_extraction() {
        // Test with multi-mount configuration
        let mut config_multi = StaticFileConfig::single("test-temp".to_string(), false);
        config_multi.add_mount("/static".to_string(), "test-temp".to_string(), false);
        let handler_multi = StaticFileHandler::new(config_multi).expect("Failed to create multi-mount handler");

        // Test mount finding
        let (mount_info, relative_path) = handler_multi.find_mount_for_path("/static/css/style.css").unwrap();
        assert_eq!(mount_info.resolved_mount.path, "/static");
        assert_eq!(relative_path, "/css/style.css");

        // Test root mount
        let (mount_info, relative_path) = handler_multi.find_mount_for_path("/some/file.txt").unwrap();
        assert_eq!(mount_info.resolved_mount.path, "/");
        assert_eq!(relative_path, "/some/file.txt");
    }
}