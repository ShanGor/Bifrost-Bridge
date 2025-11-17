use thiserror::Error;
use std::time::Duration;

#[derive(Error, Debug)]
pub enum ProxyError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("HTTP error: {0}")]
    Http(String),

    #[error("Connection error: {0}")]
    Connection(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Authentication error: {0}")]
    Auth(String),

    #[error("URL parsing error: {0}")]
    Url(#[from] url::ParseError),

    #[error("UTF-8 error: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),

    #[error("Hyper error: {0}")]
    Hyper(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("URI error: {0}")]
    Uri(String),

    // Worker separation errors
    #[error("Worker error: {0}")]
    Worker(String),

    #[error("Worker resource limit exceeded: {0}")]
    ResourceLimitExceeded(String),

    #[error("Worker isolation violation: {0}")]
    IsolationViolation(String),

    #[error("Worker creation failed: {0}")]
    WorkerCreationFailed(String),

    #[error("Connection pool exhausted: {0}")]
    ConnectionPoolExhausted(String),

    #[error("Worker health check failed: {0}")]
    HealthCheckFailed(String),

    #[error("Metrics collection error: {0}")]
    MetricsError(String),

    #[error("Resource contention detected: {0}")]
    ResourceContention(String),

    #[error("Worker shutdown timeout: {0}")]
    WorkerShutdownTimeout(String),

    #[error("Worker recovery failed: {0}")]
    WorkerRecoveryFailed(String),
}

/// Error severity levels for determining recovery actions
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ErrorSeverity {
    Low,      // Minor issues, log and continue
    Medium,   // May affect performance, attempt recovery
    High,     // Serious issues, requires immediate attention
    Critical, // System-wide issues, may require shutdown
}

impl ProxyError {
    /// Get the severity level of this error
    pub fn severity(&self) -> ErrorSeverity {
        match self {
            // Low severity - common operational issues
            ProxyError::Io(_) | ProxyError::Http(_) | ProxyError::Connection(_) |
            ProxyError::NotFound(_) | ProxyError::Uri(_) => ErrorSeverity::Low,

            // Medium severity - resource issues that can be recovered
            ProxyError::Auth(_) | ProxyError::Url(_) | ProxyError::Utf8(_) |
            ProxyError::Hyper(_) | ProxyError::Worker(_) | ProxyError::MetricsError(_) => ErrorSeverity::Medium,

            // High severity - requires intervention but system can continue
            ProxyError::Config(_) | ProxyError::ResourceLimitExceeded(_) |
            ProxyError::ConnectionPoolExhausted(_) | ProxyError::ResourceContention(_) => ErrorSeverity::High,

            // Critical severity - may require component or system shutdown
            ProxyError::WorkerCreationFailed(_) | ProxyError::IsolationViolation(_) |
            ProxyError::HealthCheckFailed(_) | ProxyError::WorkerShutdownTimeout(_) |
            ProxyError::WorkerRecoveryFailed(_) => ErrorSeverity::Critical,
        }
    }

    /// Check if this error is recoverable
    pub fn is_recoverable(&self) -> bool {
        matches!(self.severity(), ErrorSeverity::Low | ErrorSeverity::Medium)
    }

    /// Check if this error requires worker isolation
    pub fn requires_worker_isolation(&self) -> bool {
        matches!(
            self,
            ProxyError::IsolationViolation(_) |
            ProxyError::ResourceLimitExceeded(_) |
            ProxyError::HealthCheckFailed(_)
        )
    }

    /// Get suggested recovery action
    pub fn suggested_recovery(&self) -> RecoveryAction {
        match self {
            ProxyError::Io(_) => RecoveryAction::Retry,
            ProxyError::Http(_) => RecoveryAction::Retry,
            ProxyError::Connection(_) => RecoveryAction::Reconnect,
            ProxyError::Config(_) => RecoveryAction::Reconfigure,
            ProxyError::Auth(_) => RecoveryAction::Reauthenticate,
            ProxyError::Url(_) | ProxyError::Uri(_) => RecoveryAction::BadRequest,
            ProxyError::Utf8(_) => RecoveryAction::BadRequest,
            ProxyError::Hyper(_) => RecoveryAction::Retry,
            ProxyError::NotFound(_) => RecoveryAction::NotFound,
            ProxyError::Worker(_) => RecoveryAction::RestartWorker,
            ProxyError::ResourceLimitExceeded(_) => RecoveryAction::Throttle,
            ProxyError::IsolationViolation(_) => RecoveryAction::IsolateWorker,
            ProxyError::WorkerCreationFailed(_) => RecoveryAction::SkipWorker,
            ProxyError::ConnectionPoolExhausted(_) => RecoveryAction::ExpandPool,
            ProxyError::HealthCheckFailed(_) => RecoveryAction::RestartWorker,
            ProxyError::MetricsError(_) => RecoveryAction::Ignore, // Metrics errors shouldn't stop operations
            ProxyError::ResourceContention(_) => RecoveryAction::Throttle,
            ProxyError::WorkerShutdownTimeout(_) => RecoveryAction::ForceShutdown,
            ProxyError::WorkerRecoveryFailed(_) => RecoveryAction::SkipWorker,
        }
    }

    /// Get suggested delay before recovery attempt
    pub fn recovery_delay(&self) -> Duration {
        match self.severity() {
            ErrorSeverity::Low => Duration::from_millis(100),
            ErrorSeverity::Medium => Duration::from_millis(1000),
            ErrorSeverity::High => Duration::from_millis(5000),
            ErrorSeverity::Critical => Duration::from_millis(10000),
        }
    }
}

/// Recovery actions for different error types
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryAction {
    Retry,
    Reconnect,
    Reconfigure,
    Reauthenticate,
    BadRequest,
    NotFound,
    RestartWorker,
    Throttle,
    IsolateWorker,
    SkipWorker,
    ExpandPool,
    ForceShutdown,
    Ignore, // Continue without recovery attempt
}

/// Error context for better debugging and monitoring
#[derive(Debug, Clone)]
pub struct ErrorContext {
    pub component: String,
    pub operation: String,
    pub worker_id: Option<String>,
    pub proxy_type: Option<String>,
    pub connection_id: Option<String>,
    pub request_id: Option<String>,
    pub metadata: std::collections::HashMap<String, String>,
}

impl ErrorContext {
    pub fn new(component: &str, operation: &str) -> Self {
        Self {
            component: component.to_string(),
            operation: operation.to_string(),
            worker_id: None,
            proxy_type: None,
            connection_id: None,
            request_id: None,
            metadata: std::collections::HashMap::new(),
        }
    }

    pub fn with_worker_id(mut self, worker_id: &str) -> Self {
        self.worker_id = Some(worker_id.to_string());
        self
    }

    pub fn with_proxy_type(mut self, proxy_type: &str) -> Self {
        self.proxy_type = Some(proxy_type.to_string());
        self
    }

    pub fn with_connection_id(mut self, connection_id: &str) -> Self {
        self.connection_id = Some(connection_id.to_string());
        self
    }

    pub fn with_request_id(mut self, request_id: &str) -> Self {
        self.request_id = Some(request_id.to_string());
        self
    }

    pub fn with_metadata(mut self, key: &str, value: &str) -> Self {
        self.metadata.insert(key.to_string(), value.to_string());
        self
    }
}

/// Enhanced error with context and recovery information
#[derive(Debug)]
pub struct ContextualError {
    pub error: ProxyError,
    pub context: ErrorContext,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub recovery_attempts: u32,
}

impl Clone for ContextualError {
    fn clone(&self) -> Self {
        Self {
            error: ProxyError::Http(self.error.to_string()), // Convert to string for cloning
            context: self.context.clone(),
            timestamp: self.timestamp,
            recovery_attempts: self.recovery_attempts,
        }
    }
}

impl ContextualError {
    pub fn new(error: ProxyError, context: ErrorContext) -> Self {
        Self {
            error,
            context,
            timestamp: chrono::Utc::now(),
            recovery_attempts: 0,
        }
    }

    pub fn with_worker_context(error: ProxyError, component: &str, operation: &str, worker_id: &str) -> Self {
        let context = ErrorContext::new(component, operation)
            .with_worker_id(worker_id);
        Self::new(error, context)
    }

    pub fn increment_recovery_attempts(&mut self) {
        self.recovery_attempts += 1;
    }

    pub fn should_retry(&self) -> bool {
        self.error.is_recoverable() && self.recovery_attempts < 3
    }

    pub fn should_isolate_worker(&self) -> bool {
        self.error.requires_worker_isolation() && self.recovery_attempts >= 2
    }
}

impl std::fmt::Display for ContextualError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {} in {}::{} (attempts: {})",
            self.timestamp.format("%Y-%m-%d %H:%M:%S UTC"),
            self.error,
            self.context.component,
            self.context.operation,
            self.recovery_attempts
        )?;

        if let Some(worker_id) = &self.context.worker_id {
            write!(f, " [worker: {}]", worker_id)?;
        }

        if let Some(proxy_type) = &self.context.proxy_type {
            write!(f, " [type: {}]", proxy_type)?;
        }

        Ok(())
    }
}

impl std::error::Error for ContextualError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}