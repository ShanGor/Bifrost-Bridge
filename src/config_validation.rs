//! Comprehensive configuration validation for worker separation
//!
//! This module provides validation for all configuration aspects of the worker
//! separation architecture, ensuring safe and optimal operation.

use crate::common::{ProxyType, WorkerResourceLimits};
use std::collections::HashMap;

/// Comprehensive validation result
#[derive(Debug, Clone)]
pub struct ValidationResult {
    pub is_valid: bool,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    pub suggestions: Vec<String>,
}

impl ValidationResult {
    pub fn success() -> Self {
        Self {
            is_valid: true,
            errors: vec![],
            warnings: vec![],
            suggestions: vec![],
        }
    }

    pub fn failure(errors: Vec<String>) -> Self {
        Self {
            is_valid: false,
            errors,
            warnings: vec![],
            suggestions: vec![],
        }
    }

    pub fn add_error(&mut self, error: String) {
        self.errors.push(error);
        self.is_valid = false;
    }

    pub fn add_warning(&mut self, warning: String) {
        self.warnings.push(warning);
    }

    pub fn add_suggestion(&mut self, suggestion: String) {
        self.suggestions.push(suggestion);
    }
}

/// Comprehensive configuration validator for worker separation
pub struct WorkerSeparationValidator {
    forward_proxy_enabled: bool,
    reverse_proxy_enabled: bool,
    static_files_enabled: bool,
    combined_mode: bool,
    listen_addresses: HashMap<ProxyType, String>,
    resource_limits: HashMap<ProxyType, WorkerResourceLimits>,
}

impl WorkerSeparationValidator {
    pub fn new() -> Self {
        Self {
            forward_proxy_enabled: false,
            reverse_proxy_enabled: false,
            static_files_enabled: false,
            combined_mode: false,
            listen_addresses: HashMap::new(),
            resource_limits: HashMap::new(),
        }
    }

    /// Set which proxy types are enabled
    pub fn with_enabled_types(
        mut self,
        forward_proxy: bool,
        reverse_proxy: bool,
        static_files: bool,
        combined_mode: bool,
    ) -> Self {
        self.forward_proxy_enabled = forward_proxy;
        self.reverse_proxy_enabled = reverse_proxy;
        self.static_files_enabled = static_files;
        self.combined_mode = combined_mode;
        self
    }

    /// Add listen address for a proxy type
    pub fn with_listen_address(mut self, proxy_type: ProxyType, addr: String) -> Self {
        self.listen_addresses.insert(proxy_type, addr);
        self
    }

    /// Add resource limits for a proxy type
    pub fn with_resource_limits(mut self, proxy_type: ProxyType, limits: WorkerResourceLimits) -> Self {
        self.resource_limits.insert(proxy_type, limits);
        self
    }

    /// Validate the complete worker separation configuration
    pub fn validate(&self) -> ValidationResult {
        let mut result = ValidationResult::success();

        // Validate proxy type conflicts
        self.validate_proxy_type_conflicts(&mut result);

        // Validate listen addresses
        self.validate_listen_addresses(&mut result);

        // Validate resource limits
        self.validate_resource_limits(&mut result);

        // Validate performance implications
        self.validate_performance_implications(&mut result);

        // Validate security implications
        self.validate_security_implications(&mut result);

        result
    }

    /// Validate that proxy types don't conflict
    fn validate_proxy_type_conflicts(&self, result: &mut ValidationResult) {
        if self.combined_mode && (self.forward_proxy_enabled || self.reverse_proxy_enabled || self.static_files_enabled) {
            result.add_error(
                "Combined mode cannot be used together with individual proxy types".to_string()
            );
        }

        // Check for port conflicts
        let mut used_ports = std::collections::HashSet::new();
        for (proxy_type, addr) in &self.listen_addresses {
            if let Ok(port) = Self::extract_port(addr) {
                if used_ports.contains(&port) {
                    result.add_error(format!(
                        "Port {} is used by multiple proxy types (conflict detected for {:?})",
                        port, proxy_type
                    ));
                }
                used_ports.insert(port);
            }
        }
    }

    /// Validate listen addresses
    fn validate_listen_addresses(&self, result: &mut ValidationResult) {
        for (proxy_type, addr) in &self.listen_addresses {
            if addr.is_empty() {
                result.add_error(format!("Listen address cannot be empty for {:?}", proxy_type));
                continue;
            }

            // Validate address format
            if let Err(e) = Self::validate_address(addr) {
                result.add_error(format!(
                    "Invalid listen address for {:?}: {} - {}",
                    proxy_type, addr, e
                ));
            }

            // Check for privileged ports
            if let Ok(port) = Self::extract_port(addr) {
                if port < 1024 {
                    result.add_warning(format!(
                        "Using privileged port {} for {:?} - may require elevated privileges",
                        port, proxy_type
                    ));
                }
            }
        }
    }

    /// Validate resource limits
    fn validate_resource_limits(&self, result: &mut ValidationResult) {
        for (proxy_type, limits) in &self.resource_limits {
            // Basic validation is already done in WorkerResourceLimits::validate()
            if let Err(e) = limits.validate() {
                result.add_error(format!("Invalid resource limits for {:?}: {}", proxy_type, e));
            }

            // Additional cross-proxy validation
            self.validate_resource_limits_for_type(proxy_type, limits, result);
        }
    }

    /// Validate resource limits specific to proxy type
    fn validate_resource_limits_for_type(
        &self,
        proxy_type: &ProxyType,
        limits: &WorkerResourceLimits,
        result: &mut ValidationResult,
    ) {
        match proxy_type {
            ProxyType::ForwardProxy => {
                if limits.max_connections > 10000 {
                    result.add_warning(format!(
                        "High connection limit for ForwardProxy ({}): consider using load balancing",
                        limits.max_connections
                    ));
                }

                if limits.max_memory_mb < 256 {
                    result.add_warning(
                        "Low memory limit for ForwardProxy may impact performance with many connections".to_string()
                    );
                }
            }

            ProxyType::ReverseProxy => {
                if limits.max_memory_mb < 512 {
                    result.add_warning(
                        "ReverseProxy typically needs more memory for caching and request buffering".to_string()
                    );
                }

                if limits.max_requests_per_second < 5000 {
                    result.add_warning(
                        "Low RPS limit for ReverseProxy may impact backend performance".to_string()
                    );
                }
            }

            ProxyType::StaticFiles => {
                if limits.max_file_size_mb < 1000 {
                    result.add_warning(
                        "Small file size limit for StaticFiles may prevent serving large assets".to_string()
                    );
                }

                if limits.max_memory_mb < 512 {
                    result.add_suggestion(
                        "Consider increasing memory limit for StaticFiles to improve caching performance".to_string()
                    );
                }
            }

            ProxyType::Combined => {
                // Combined mode should have higher limits
                if limits.max_connections < 2000 {
                    result.add_warning(
                        "Combined mode should have higher connection limits for optimal performance".to_string()
                    );
                }

                if limits.max_memory_mb < 2048 {
                    result.add_warning(
                        "Combined mode typically requires more memory for all proxy types".to_string()
                    );
                }
            }
        }
    }

    /// Validate performance implications
    fn validate_performance_implications(&self, result: &mut ValidationResult) {
        let total_connections: usize = self.resource_limits.values()
            .map(|limits| limits.max_connections)
            .sum();

        if total_connections > 10000 {
            result.add_warning(format!(
                "High total connection limit ({}): ensure system has sufficient resources",
                total_connections
            ));
        }

        let total_memory: usize = self.resource_limits.values()
            .map(|limits| limits.max_memory_mb)
            .sum();

        if total_memory > 4096 {
            result.add_warning(format!(
                "High memory allocation ({} MB): monitor system memory usage",
                total_memory
            ));
        }

        // Check for potential resource contention
        if self.forward_proxy_enabled && self.reverse_proxy_enabled {
            let forward_cpu = self.resource_limits.get(&ProxyType::ForwardProxy)
                .map(|l| l.max_cpu_percent)
                .unwrap_or(0.0);
            let reverse_cpu = self.resource_limits.get(&ProxyType::ReverseProxy)
                .map(|l| l.max_cpu_percent)
                .unwrap_or(0.0);

            if forward_cpu + reverse_cpu > 150.0 {
                result.add_warning(
                    "High combined CPU limits may cause resource contention between ForwardProxy and ReverseProxy".to_string()
                );
            }
        }
    }

    /// Validate security implications
    fn validate_security_implications(&self, result: &mut ValidationResult) {
        for (proxy_type, addr) in &self.listen_addresses {
            if let Some(host) = Self::extract_host(addr) {
                // Check for potentially insecure configurations
                if host == "0.0.0.0" || host == "::" {
                    result.add_warning(format!(
                        "Binding to all interfaces ({} for {:?}) exposes service to external networks",
                        host, proxy_type
                    ));

                    if *proxy_type == ProxyType::ForwardProxy {
                        result.add_suggestion(
                            "Consider binding ForwardProxy to internal interfaces only for better security".to_string()
                        );
                    }
                }
            }
        }

        // Check for timeout configurations that could lead to DoS
        for (proxy_type, limits) in &self.resource_limits {
            if limits.connection_timeout_secs > 300 {
                result.add_warning(format!(
                    "Long connection timeout ({}) for {:?} may be exploited for DoS attacks",
                    limits.connection_timeout_secs, proxy_type
                ));
            }

            if limits.idle_timeout_secs > 600 {
                result.add_warning(format!(
                    "Long idle timeout ({}) for {:?} may lead to resource exhaustion",
                    limits.idle_timeout_secs, proxy_type
                ));
            }
        }
    }

    /// Extract port from address string
    fn extract_port(addr: &str) -> Result<u16, String> {
        let parts: Vec<&str> = addr.split(':').collect();
        if parts.len() != 2 {
            return Err("Invalid address format".to_string());
        }

        parts[1].parse::<u16>()
            .map_err(|_| "Invalid port number".to_string())
    }

    /// Extract host from address string
    fn extract_host(addr: &str) -> Option<String> {
        let parts: Vec<&str> = addr.split(':').collect();
        if parts.len() >= 2 {
            Some(parts[0].to_string())
        } else {
            None
        }
    }

    /// Validate address format
    fn validate_address(addr: &str) -> Result<(), String> {
        if addr.is_empty() {
            return Err("Address cannot be empty".to_string());
        }

        let parts: Vec<&str> = addr.split(':').collect();
        if parts.len() != 2 {
            return Err("Address must be in format 'host:port'".to_string());
        }

        // Validate host part
        let host = parts[0];
        if host.is_empty() {
            return Err("Host cannot be empty".to_string());
        }

        // Validate port part
        let port_str = parts[1];
        let port = port_str.parse::<u16>()
            .map_err(|_| "Invalid port number".to_string())?;

        if port == 0 {
            return Err("Port 0 is reserved".to_string());
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validation_success() {
        let validator = WorkerSeparationValidator::new()
            .with_enabled_types(true, false, false, false)
            .with_listen_address(ProxyType::ForwardProxy, "127.0.0.1:3128".to_string())
            .with_resource_limits(ProxyType::ForwardProxy, WorkerResourceLimits::default());

        let result = validator.validate();
        assert!(result.is_valid);
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_validation_port_conflict() {
        let validator = WorkerSeparationValidator::new()
            .with_enabled_types(true, true, false, false)
            .with_listen_address(ProxyType::ForwardProxy, "127.0.0.1:8080".to_string())
            .with_listen_address(ProxyType::ReverseProxy, "0.0.0.0:8080".to_string());

        let result = validator.validate();
        assert!(!result.is_valid);
        assert!(!result.errors.is_empty());
    }

    #[test]
    fn test_validation_invalid_address() {
        let validator = WorkerSeparationValidator::new()
            .with_enabled_types(true, false, false, false)
            .with_listen_address(ProxyType::ForwardProxy, "invalid-address".to_string());

        let result = validator.validate();
        assert!(!result.is_valid);
    }
}