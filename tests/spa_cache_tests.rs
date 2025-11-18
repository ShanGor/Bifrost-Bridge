//! Integration tests for SPA cache control functionality
//!
//! These tests verify that SPA files (index files and fallbacks) receive
//! no-cache headers while regular static assets retain normal caching.

use bifrost_bridge::config::{StaticFileConfig, StaticMount};
use bifrost_bridge::static_files::StaticFileHandler;
use std::fs;
use tempfile::TempDir;

/// Test that SPA index files receive no-cache headers
#[tokio::test]
async fn test_spa_index_files_no_cache() {
    let temp_dir = TempDir::new().unwrap();
    let static_dir = temp_dir.path();

    // Create an index.html file
    let index_content = "<html><head><title>SPA App</title></head><body>Hello SPA</body></html>";
    fs::write(static_dir.join("index.html"), index_content).unwrap();

    // Create a regular asset file
    let asset_content = "body { background: #fff; }";
    fs::write(static_dir.join("styles.css"), asset_content).unwrap();

    // Configure SPA mode with custom index files
    let config = StaticFileConfig {
        mounts: vec![StaticMount {
            path: "/".to_string(),
            root_dir: static_dir.to_string_lossy().to_string(),
            enable_directory_listing: None,
            index_files: Some(vec!["index.html".to_string(), "main.htm".to_string()]),
            spa_mode: Some(true),
            spa_fallback_file: Some("index.html".to_string()),
            no_cache_files: None,
            cache_millisecs: None,
        }],
        enable_directory_listing: false,
        index_files: vec!["index.html".to_string(), "index.htm".to_string()],
        spa_mode: true,
        spa_fallback_file: "index.html".to_string(),
        worker_threads: None,
        custom_mime_types: std::collections::HashMap::new(),
        no_cache_files: vec![],
        cache_millisecs: 3600,
    };

    let handler = StaticFileHandler::new(config).unwrap();

    // Get mount info for SPA context
    let (mount_info, _) = handler.find_mount_for_path("/index.html").unwrap();

    // Test index.html file (should have no-cache)
    let index_path = static_dir.join("index.html");
    let index_response = handler.handle_file_with_mount_info(&index_path, false, Some(mount_info), false).await.unwrap();

    let cache_control = index_response.headers().get("Cache-Control").unwrap();
    assert_eq!(cache_control, "no-cache, no-store, must-revalidate");

    // Test regular CSS file (should have normal cache)
    let css_path = static_dir.join("styles.css");
    let css_response = handler.handle_file_with_mount_info(&css_path, false, Some(mount_info), false).await.unwrap();

    let css_cache_control = css_response.headers().get("Cache-Control").unwrap();
    assert_eq!(css_cache_control, "public, max-age=3600");
}

/// Test that SPA fallback responses receive no-cache headers
#[tokio::test]
async fn test_spa_fallback_no_cache() {
    let temp_dir = TempDir::new().unwrap();
    let static_dir = temp_dir.path();

    // Create an index.html file for fallback
    let index_content = "<html><head><title>Fallback</title></head><body>SPA Fallback</body></html>";
    fs::write(static_dir.join("index.html"), index_content).unwrap();

    // Configure SPA mode
    let config = StaticFileConfig::single(static_dir.to_string_lossy().to_string(), true);
    let handler = StaticFileHandler::new(config).unwrap();

    // Simulate a SPA fallback request
    let index_path = static_dir.join("index.html");

    // Get mount info for SPA fallback
    let (mount_info, _) = handler.find_mount_for_path("/non-existent-route").unwrap();

    let fallback_response = handler.handle_file_with_mount_info(&index_path, false, Some(mount_info), true).await.unwrap();

    let cache_control = fallback_response.headers().get("Cache-Control").unwrap();
    assert_eq!(cache_control, "no-cache, no-store, must-revalidate");
}

/// Test that regular static files maintain normal caching
#[tokio::test]
async fn test_regular_static_files_normal_cache() {
    let temp_dir = TempDir::new().unwrap();
    let static_dir = temp_dir.path();

    // Create various static asset files
    fs::write(static_dir.join("app.js"), "console.log('Hello');").unwrap();
    fs::write(static_dir.join("style.css"), "body { margin: 0; }").unwrap();
    fs::write(static_dir.join("image.png"), b"fake png data").unwrap();

    // Configure non-SPA mode
    let config = StaticFileConfig::single(static_dir.to_string_lossy().to_string(), false);
    let handler = StaticFileHandler::new(config).unwrap();

    // Test that regular assets get normal cache headers
    for file_name in ["app.js", "style.css", "image.png"] {
        let file_path = static_dir.join(file_name);
        let response = handler.handle_file_with_mount_info(&file_path, false, None, false).await.unwrap();

        let cache_control = response.headers().get("Cache-Control").unwrap();
        assert_eq!(cache_control, "public, max-age=3600", "File {} should have normal cache", file_name);
    }
}

/// Test that custom index files in SPA mode get no-cache
#[tokio::test]
async fn test_custom_index_files_spa_no_cache() {
    let temp_dir = TempDir::new().unwrap();
    let static_dir = temp_dir.path();

    // Create custom index files
    fs::write(static_dir.join("main.htm"), "<html><body>Custom Index</body></html>").unwrap();
    fs::write(static_dir.join("app.html"), "<html><body>App Index</body></html>").unwrap();

    // Configure SPA mode with custom index files
    let config = StaticFileConfig {
        mounts: vec![StaticMount {
            path: "/".to_string(),
            root_dir: static_dir.to_string_lossy().to_string(),
            enable_directory_listing: None,
            index_files: Some(vec!["main.htm".to_string(), "app.html".to_string()]),
            spa_mode: Some(true),
            spa_fallback_file: Some("main.htm".to_string()),
            no_cache_files: None,
            cache_millisecs: None,
        }],
        enable_directory_listing: false,
        index_files: vec!["main.htm".to_string(), "app.html".to_string()],
        spa_mode: true,
        spa_fallback_file: "main.htm".to_string(),
        worker_threads: None,
        custom_mime_types: std::collections::HashMap::new(),
        no_cache_files: vec![],
        cache_millisecs: 3600,
    };

    let handler = StaticFileHandler::new(config).unwrap();
    let (mount_info, _) = handler.find_mount_for_path("/").unwrap();

    // Test that custom index files get no-cache headers in SPA mode
    for file_name in ["main.htm", "app.html"] {
        let file_path = static_dir.join(file_name);
        let response = handler.handle_file_with_mount_info(&file_path, false, Some(mount_info), false).await.unwrap();

        let cache_control = response.headers().get("Cache-Control").unwrap();
        assert_eq!(cache_control, "no-cache, no-store, must-revalidate",
                 "Custom index file {} should have no-cache in SPA mode", file_name);
    }
}

/// Test that non-SPA mode maintains normal caching for all files
#[tokio::test]
async fn test_non_spa_mode_normal_cache() {
    let temp_dir = TempDir::new().unwrap();
    let static_dir = temp_dir.path();

    // Create various files including index.html
    fs::write(static_dir.join("index.html"), "<html><body>Non-SPA</body></html>").unwrap();
    fs::write(static_dir.join("app.js"), "console.log('test');").unwrap();
    fs::write(static_dir.join("style.css"), "body { color: red; }").unwrap();

    // Configure non-SPA mode
    let config = StaticFileConfig::single(static_dir.to_string_lossy().to_string(), false);
    let handler = StaticFileHandler::new(config).unwrap();

    // Test that all files get normal cache headers in non-SPA mode
    for file_name in ["index.html", "app.js", "style.css"] {
        let file_path = static_dir.join(file_name);
        let response = handler.handle_file_with_mount_info(&file_path, false, None, false).await.unwrap();

        let cache_control = response.headers().get("Cache-Control").unwrap();
        assert_eq!(cache_control, "public, max-age=3600",
                 "File {} should have normal cache in non-SPA mode", file_name);
    }
}

/// Test that fallback file detection works correctly
#[tokio::test]
async fn test_spa_fallback_file_detection() {
    let temp_dir = TempDir::new().unwrap();
    let static_dir = temp_dir.path();

    // Create custom fallback file
    fs::write(static_dir.join("fallback.html"), "<html><body>Fallback</body></html>").unwrap();

    // Configure SPA mode with custom fallback file
    let config = StaticFileConfig {
        mounts: vec![StaticMount {
            path: "/".to_string(),
            root_dir: static_dir.to_string_lossy().to_string(),
            enable_directory_listing: None,
            index_files: None,
            spa_mode: Some(true),
            spa_fallback_file: Some("fallback.html".to_string()),
            no_cache_files: None,
            cache_millisecs: None,
        }],
        enable_directory_listing: false,
        index_files: vec!["index.html".to_string()],
        spa_mode: true,
        spa_fallback_file: "fallback.html".to_string(),
        worker_threads: None,
        custom_mime_types: std::collections::HashMap::new(),
        no_cache_files: vec![],
        cache_millisecs: 3600,
    };

    let handler = StaticFileHandler::new(config).unwrap();
    let (mount_info, _) = handler.find_mount_for_path("/").unwrap();

    // Test that custom fallback file gets no-cache headers
    let fallback_path = static_dir.join("fallback.html");
    let response = handler.handle_file_with_mount_info(&fallback_path, false, Some(mount_info), true).await.unwrap();

    let cache_control = response.headers().get("Cache-Control").unwrap();
    assert_eq!(cache_control, "no-cache, no-store, must-revalidate");
}

/// Test HEAD requests maintain cache headers correctly
#[tokio::test]
async fn test_spa_cache_head_requests() {
    let temp_dir = TempDir::new().unwrap();
    let static_dir = temp_dir.path();

    // Create index.html file
    fs::write(static_dir.join("index.html"), "<html><body>SPA</body></html>").unwrap();

    // Configure SPA mode
    let config = StaticFileConfig::single(static_dir.to_string_lossy().to_string(), true);
    let handler = StaticFileHandler::new(config).unwrap();
    let (mount_info, _) = handler.find_mount_for_path("/").unwrap();

    // Test HEAD request to SPA index file
    let index_path = static_dir.join("index.html");
    let response = handler.handle_file_with_mount_info(&index_path, true, Some(mount_info), false).await.unwrap();

    let cache_control = response.headers().get("Cache-Control").unwrap();
    assert_eq!(cache_control, "no-cache, no-store, must-revalidate");

    // Verify HEAD response has proper headers (Content-Length should be present but body empty for HEAD)
    let content_length = response.headers().get("Content-Length").unwrap();
    assert!(!content_length.is_empty(), "HEAD response should have Content-Length header");
}

/// Test that custom no_cache_files patterns work correctly
#[tokio::test]
async fn test_custom_no_cache_files_patterns() {
    let temp_dir = TempDir::new().unwrap();
    let static_dir = temp_dir.path();

    // Create various files
    fs::write(static_dir.join("app.js"), "console.log('app');").unwrap();
    fs::write(static_dir.join("config.json"), "{\"env\":\"dev\"}").unwrap();
    fs::write(static_dir.join("style.css"), "body { margin: 0; }").unwrap();
    fs::write(static_dir.join("image.png"), b"fake png data").unwrap();

    // Configure with custom no_cache_files patterns
    let config = StaticFileConfig {
        mounts: vec![StaticMount {
            path: "/".to_string(),
            root_dir: static_dir.to_string_lossy().to_string(),
            enable_directory_listing: None,
            index_files: None,
            spa_mode: Some(false), // Non-SPA mode to test no_cache_files independently
            spa_fallback_file: None,
            no_cache_files: Some(vec!["*.js".to_string(), "config.json".to_string()]),
            cache_millisecs: None,
        }],
        enable_directory_listing: false,
        index_files: vec!["index.html".to_string()],
        spa_mode: false,
        spa_fallback_file: "index.html".to_string(),
        worker_threads: None,
        custom_mime_types: std::collections::HashMap::new(),
        no_cache_files: vec![],
        cache_millisecs: 7200, // 2 hours
    };

    let handler = StaticFileHandler::new(config).unwrap();
    let (mount_info, _) = handler.find_mount_for_path("/").unwrap();

    // Test JavaScript files (*.js pattern) should have no-cache
    let js_path = static_dir.join("app.js");
    let js_response = handler.handle_file_with_mount_info(&js_path, false, Some(mount_info), false).await.unwrap();
    let js_cache_control = js_response.headers().get("Cache-Control").unwrap();
    assert_eq!(js_cache_control, "no-cache, no-store, must-revalidate");

    // Test config.json (exact match) should have no-cache
    let json_path = static_dir.join("config.json");
    let json_response = handler.handle_file_with_mount_info(&json_path, false, Some(mount_info), false).await.unwrap();
    let json_cache_control = json_response.headers().get("Cache-Control").unwrap();
    assert_eq!(json_cache_control, "no-cache, no-store, must-revalidate");

    // Test CSS file (not in no_cache_files) should have normal cache
    let css_path = static_dir.join("style.css");
    let css_response = handler.handle_file_with_mount_info(&css_path, false, Some(mount_info), false).await.unwrap();
    let css_cache_control = css_response.headers().get("Cache-Control").unwrap();
    assert_eq!(css_cache_control, "public, max-age=7200");

    // Test PNG file (not in no_cache_files) should have normal cache
    let png_path = static_dir.join("image.png");
    let png_response = handler.handle_file_with_mount_info(&png_path, false, Some(mount_info), false).await.unwrap();
    let png_cache_control = png_response.headers().get("Cache-Control").unwrap();
    assert_eq!(png_cache_control, "public, max-age=7200");
}

/// Test that custom cache_millisecs configuration works correctly
#[tokio::test]
async fn test_custom_cache_millisecs_configuration() {
    let temp_dir = TempDir::new().unwrap();
    let static_dir = temp_dir.path();

    // Create test files
    fs::write(static_dir.join("app.js"), "console.log('test');").unwrap();
    fs::write(static_dir.join("style.css"), "body { color: blue; }").unwrap();

    // Configure with custom cache duration (30 minutes = 1800 seconds)
    let config = StaticFileConfig {
        mounts: vec![StaticMount {
            path: "/".to_string(),
            root_dir: static_dir.to_string_lossy().to_string(),
            enable_directory_listing: None,
            index_files: None,
            spa_mode: Some(false),
            spa_fallback_file: None,
            no_cache_files: None,
            cache_millisecs: Some(1800), // 30 minutes
        }],
        enable_directory_listing: false,
        index_files: vec!["index.html".to_string()],
        spa_mode: false,
        spa_fallback_file: "index.html".to_string(),
        worker_threads: None,
        custom_mime_types: std::collections::HashMap::new(),
        no_cache_files: vec![],
        cache_millisecs: 3600, // Global default (should be overridden by mount)
    };

    let handler = StaticFileHandler::new(config).unwrap();
    let (mount_info, _) = handler.find_mount_for_path("/").unwrap();

    // Test that regular files use the custom cache duration from mount
    for file_name in ["app.js", "style.css"] {
        let file_path = static_dir.join(file_name);
        let response = handler.handle_file_with_mount_info(&file_path, false, Some(mount_info), false).await.unwrap();
        let cache_control = response.headers().get("Cache-Control").unwrap();
        assert_eq!(cache_control, "public, max-age=1800", "File {} should use custom cache duration", file_name);
    }
}

/// Test that global cache_millisecs is used when mount doesn't specify it
#[tokio::test]
async fn test_global_cache_millisecs_fallback() {
    let temp_dir = TempDir::new().unwrap();
    let static_dir = temp_dir.path();

    // Create test file
    fs::write(static_dir.join("test.txt"), "Hello World").unwrap();

    // Configure with only global cache_millisecs
    let config = StaticFileConfig {
        mounts: vec![StaticMount {
            path: "/".to_string(),
            root_dir: static_dir.to_string_lossy().to_string(),
            enable_directory_listing: None,
            index_files: None,
            spa_mode: Some(false),
            spa_fallback_file: None,
            no_cache_files: None,
            cache_millisecs: None, // Mount doesn't specify, should inherit from global
        }],
        enable_directory_listing: false,
        index_files: vec!["index.html".to_string()],
        spa_mode: false,
        spa_fallback_file: "index.html".to_string(),
        worker_threads: None,
        custom_mime_types: std::collections::HashMap::new(),
        no_cache_files: vec![],
        cache_millisecs: 14400, // 4 hours
    };

    let handler = StaticFileHandler::new(config).unwrap();
    let (mount_info, _) = handler.find_mount_for_path("/").unwrap();

    // Test that file inherits global cache duration
    let file_path = static_dir.join("test.txt");
    let response = handler.handle_file_with_mount_info(&file_path, false, Some(mount_info), false).await.unwrap();
    let cache_control = response.headers().get("Cache-Control").unwrap();
    assert_eq!(cache_control, "public, max-age=14400");
}

/// Test case-insensitive pattern matching for no_cache_files
#[tokio::test]
async fn test_no_cache_files_case_insensitive() {
    let temp_dir = TempDir::new().unwrap();
    let static_dir = temp_dir.path();

    // Create files with mixed case
    fs::write(static_dir.join("APP.JS"), "console.log('uppercase');").unwrap();
    fs::write(static_dir.join("Config.JSON"), "{\"env\":\"prod\"}").unwrap();

    // Configure with lowercase patterns
    let config = StaticFileConfig {
        mounts: vec![StaticMount {
            path: "/".to_string(),
            root_dir: static_dir.to_string_lossy().to_string(),
            enable_directory_listing: None,
            index_files: None,
            spa_mode: Some(false),
            spa_fallback_file: None,
            no_cache_files: Some(vec!["*.js".to_string(), "config.json".to_string()]),
            cache_millisecs: None,
        }],
        enable_directory_listing: false,
        index_files: vec!["index.html".to_string()],
        spa_mode: false,
        spa_fallback_file: "index.html".to_string(),
        worker_threads: None,
        custom_mime_types: std::collections::HashMap::new(),
        no_cache_files: vec![],
        cache_millisecs: 3600,
    };

    let handler = StaticFileHandler::new(config).unwrap();
    let (mount_info, _) = handler.find_mount_for_path("/").unwrap();

    // Test that uppercase files match lowercase patterns
    let js_path = static_dir.join("APP.JS");
    let js_response = handler.handle_file_with_mount_info(&js_path, false, Some(mount_info), false).await.unwrap();
    let js_cache_control = js_response.headers().get("Cache-Control").unwrap();
    assert_eq!(js_cache_control, "no-cache, no-store, must-revalidate");

    let json_path = static_dir.join("Config.JSON");
    let json_response = handler.handle_file_with_mount_info(&json_path, false, Some(mount_info), false).await.unwrap();
    let json_cache_control = json_response.headers().get("Cache-Control").unwrap();
    assert_eq!(json_cache_control, "no-cache, no-store, must-revalidate");
}