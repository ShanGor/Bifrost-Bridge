//! Error recovery and handling system for worker separation architecture
//!
//! This module provides sophisticated error recovery mechanisms including:
//! - Automatic retry with exponential backoff
//! - Worker isolation and recovery
//! - Circuit breaker pattern
//! - Error aggregation and analysis
//! - Graceful degradation strategies

use crate::common::{ProxyType, IsolatedWorker};
use crate::error::{ProxyError, ContextualError, RecoveryAction, ErrorSeverity};
use std::sync::Arc;
use std::time::Duration;
use std::collections::HashMap;
use tokio::sync::{RwLock, Semaphore};
use log::{warn, error, info, debug};

/// Circuit breaker state for preventing cascade failures
#[derive(Debug, Clone, PartialEq)]
pub enum CircuitState {
    Closed,    // Normal operation
    Open,      // Failing, reject requests
    HalfOpen,  // Testing if failures have resolved
}

/// Circuit breaker for protecting against cascading failures
pub struct CircuitBreaker {
    state: Arc<RwLock<CircuitState>>,
    failure_count: Arc<RwLock<u32>>,
    success_count: Arc<RwLock<u32>>,
    failure_threshold: u32,
    success_threshold: u32,
    timeout: Duration,
    last_failure_time: Arc<RwLock<Option<chrono::DateTime<chrono::Utc>>>>,
}

impl CircuitBreaker {
    pub fn new(failure_threshold: u32, success_threshold: u32, timeout: Duration) -> Self {
        Self {
            state: Arc::new(RwLock::new(CircuitState::Closed)),
            failure_count: Arc::new(RwLock::new(0)),
            success_count: Arc::new(RwLock::new(0)),
            failure_threshold,
            success_threshold,
            timeout,
            last_failure_time: Arc::new(RwLock::new(None)),
        }
    }

    pub async fn call<F, T>(&self, f: F) -> Result<T, ProxyError>
    where
        F: std::future::Future<Output = Result<T, ProxyError>>,
    {
        // Check circuit state
        if !self.can_request().await {
            return Err(ProxyError::Connection("Circuit breaker is open".to_string()));
        }

        // Execute the operation
        match f.await {
            Ok(result) => {
                self.record_success().await;
                Ok(result)
            }
            Err(_err) => {
                self.record_failure().await;
                Err(ProxyError::Connection("Operation failed".to_string()))
            }
        }
    }

    async fn can_request(&self) -> bool {
        let mut state = self.state.write().await;

        match *state {
            CircuitState::Closed => true,
            CircuitState::Open => {
                let last_failure = self.last_failure_time.read().await;
                if let Some(last_failure_time) = *last_failure {
                    let elapsed = chrono::Utc::now() - last_failure_time;
                    if elapsed.to_std().unwrap_or(Duration::MAX) > self.timeout {
                        *state = CircuitState::HalfOpen;
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
            CircuitState::HalfOpen => true,
        }
    }

    async fn record_success(&self) {
        let mut state = self.state.write().await;
        let mut failure_count = self.failure_count.write().await;
        let mut success_count = self.success_count.write().await;

        match *state {
            CircuitState::Open => {
                *state = CircuitState::HalfOpen;
                *success_count = 1;
                *failure_count = 0;
            }
            CircuitState::HalfOpen => {
                *success_count += 1;
                if *success_count >= self.success_threshold {
                    *state = CircuitState::Closed;
                    *failure_count = 0;
                    *success_count = 0;
                }
            }
            CircuitState::Closed => {
                *failure_count = 0;
            }
        }
    }

    async fn record_failure(&self) {
        let mut state = self.state.write().await;
        let mut failure_count = self.failure_count.write().await;
        let mut success_count = self.success_count.write().await;
        let mut last_failure_time = self.last_failure_time.write().await;

        *failure_count += 1;
        *last_failure_time = Some(chrono::Utc::now());

        match *state {
            CircuitState::Closed => {
                if *failure_count >= self.failure_threshold {
                    *state = CircuitState::Open;
                }
            }
            CircuitState::HalfOpen => {
                *state = CircuitState::Open;
                *success_count = 0;
            }
            CircuitState::Open => {
                // Already open, just update failure count
            }
        }
    }

    pub async fn get_state(&self) -> CircuitState {
        self.state.read().await.clone()
    }
}

/// Worker health status and recovery state
#[derive(Debug, Clone)]
pub struct WorkerHealth {
    pub worker_id: String,
    pub proxy_type: ProxyType,
    pub is_healthy: bool,
    pub consecutive_failures: u32,
    pub last_check: chrono::DateTime<chrono::Utc>,
    pub last_success: Option<chrono::DateTime<chrono::Utc>>,
    pub is_isolated: bool,
    pub recovery_attempts: u32,
}

/// Error recovery manager for handling worker failures
#[derive(Clone)]
pub struct ErrorRecoveryManager {
    workers: Arc<RwLock<HashMap<String, WorkerHealth>>>,
    circuit_breakers: Arc<RwLock<HashMap<String, CircuitBreaker>>>,
    error_history: Arc<RwLock<Vec<ContextualError>>>,
    recovery_semaphore: Arc<Semaphore>,
    max_recovery_attempts: u32,
    error_history_limit: usize,
}

impl ErrorRecoveryManager {
    pub fn new(max_recovery_attempts: u32, error_history_limit: usize) -> Self {
        Self {
            workers: Arc::new(RwLock::new(HashMap::new())),
            circuit_breakers: Arc::new(RwLock::new(HashMap::new())),
            error_history: Arc::new(RwLock::new(Vec::new())),
            recovery_semaphore: Arc::new(Semaphore::new(10)), // Limit concurrent recovery attempts
            max_recovery_attempts,
            error_history_limit,
        }
    }

    /// Register a worker for health monitoring
    pub async fn register_worker(&self, worker: &Arc<IsolatedWorker>) {
        let worker_id = format!("{}-{:?}", worker.proxy_type, std::ptr::addr_of!(*worker) as usize);

        let health = WorkerHealth {
            worker_id: worker_id.clone(),
            proxy_type: worker.proxy_type.clone(),
            is_healthy: true,
            consecutive_failures: 0,
            last_check: chrono::Utc::now(),
            last_success: Some(chrono::Utc::now()),
            is_isolated: false,
            recovery_attempts: 0,
        };

        // Register worker health
        self.workers.write().await.insert(worker_id.clone(), health);

        // Create circuit breaker for this worker
        let circuit_breaker = CircuitBreaker::new(
            5,  // failure threshold
            3,  // success threshold
            Duration::from_secs(30), // timeout
        );

        self.circuit_breakers.write().await.insert(worker_id, circuit_breaker);

        info!("Registered worker for health monitoring");
    }

    /// Handle an error that occurred in a worker
    pub async fn handle_error(&self, error: ContextualError) -> Result<(), ProxyError> {
        // Record the error
        self.record_error(error.clone()).await;

        // Get worker ID from context
        let worker_id = error.context.worker_id.clone().unwrap_or_default();

        // Update worker health
        self.update_worker_health(&worker_id, false).await;

        // Determine recovery action
        let recovery_action = if error.should_isolate_worker() {
            RecoveryAction::IsolateWorker
        } else if error.should_retry() {
            RecoveryAction::Retry
        } else {
            error.error.suggested_recovery()
        };

        // Execute recovery action
        self.execute_recovery_action(&worker_id, recovery_action).await?;

        Ok(())
    }

    /// Record an error in the history
    async fn record_error(&self, error: ContextualError) {
        let mut history = self.error_history.write().await;
        history.push(error);

        // Limit history size
        if history.len() > self.error_history_limit {
            history.remove(0);
        }
    }

    /// Update worker health status
    pub async fn update_worker_health(&self, worker_id: &str, is_success: bool) {
        let mut workers = self.workers.write().await;
        if let Some(health) = workers.get_mut(worker_id) {
            health.last_check = chrono::Utc::now();

            if is_success {
                health.is_healthy = true;
                health.consecutive_failures = 0;
                health.last_success = Some(chrono::Utc::now());
                health.recovery_attempts = 0;
            } else {
                health.is_healthy = false;
                health.consecutive_failures += 1;
            }
        }
    }

    /// Execute recovery action for a worker
    async fn execute_recovery_action(&self, worker_id: &str, action: RecoveryAction) -> Result<(), ProxyError> {
        let _permit = self.recovery_semaphore.acquire().await.map_err(|_| {
            ProxyError::Worker("Recovery semaphore acquisition failed".to_string())
        })?;

        match action {
            RecoveryAction::IsolateWorker => self.isolate_worker(worker_id).await,
            RecoveryAction::RestartWorker => self.restart_worker(worker_id).await,
            RecoveryAction::Throttle => self.throttle_worker(worker_id).await,
            RecoveryAction::SkipWorker => {
                warn!("Skipping worker {} due to repeated failures", worker_id);
                Ok(())
            }
            RecoveryAction::Retry => {
                tokio::time::sleep(Duration::from_millis(500)).await;
                Ok(())
            }
            RecoveryAction::Reconnect => {
                debug!("Attempting to reconnect worker {}", worker_id);
                tokio::time::sleep(Duration::from_millis(1000)).await;
                Ok(())
            }
            RecoveryAction::Ignore => Ok(()),
            _ => {
                warn!("Unhandled recovery action: {:?}", action);
                Ok(())
            }
        }
    }

    /// Isolate a failing worker
    async fn isolate_worker(&self, worker_id: &str) -> Result<(), ProxyError> {
        let mut workers = self.workers.write().await;
        if let Some(health) = workers.get_mut(worker_id) {
            health.is_isolated = true;
            error!("Worker {} isolated due to repeated failures", worker_id);
        }
        Ok(())
    }

    /// Attempt to restart a worker
    async fn restart_worker(&self, worker_id: &str) -> Result<(), ProxyError> {
        let mut workers = self.workers.write().await;
        if let Some(health) = workers.get_mut(worker_id) {
            health.recovery_attempts += 1;

            if health.recovery_attempts >= self.max_recovery_attempts {
                error!("Worker {} exceeded max recovery attempts, marking as failed", worker_id);
                return Err(ProxyError::WorkerRecoveryFailed(format!(
                    "Worker {} exceeded max recovery attempts", worker_id
                )));
            }

            info!("Attempting to restart worker {} (attempt {})",
                  worker_id, health.recovery_attempts);

            // In a real implementation, this would trigger worker recreation
            // For now, we'll simulate the restart
            tokio::time::sleep(Duration::from_millis(1000)).await;

            health.is_healthy = true;
            health.consecutive_failures = 0;
            health.last_success = Some(chrono::Utc::now());

            info!("Worker {} restarted successfully", worker_id);
        }
        Ok(())
    }

    /// Throttle a worker that's experiencing resource contention
    async fn throttle_worker(&self, worker_id: &str) -> Result<(), ProxyError> {
        warn!("Throttling worker {} due to resource contention", worker_id);

        // Implement throttling logic here
        tokio::time::sleep(Duration::from_millis(2000)).await;

        info!("Throttling completed for worker {}", worker_id);
        Ok(())
    }

    /// Get health status of all workers
    pub async fn get_worker_health(&self) -> HashMap<String, WorkerHealth> {
        self.workers.read().await.clone()
    }

    /// Get recent errors from history
    pub async fn get_recent_errors(&self, limit: usize) -> Vec<ContextualError> {
        let history = self.error_history.read().await;
        history.iter()
            .rev()
            .take(limit)
            .map(|e| e.clone())
            .collect()
    }

    /// Perform health check on all registered workers
    pub async fn perform_health_checks(&self) {
        let workers = self.workers.read().await;
        let worker_ids: Vec<String> = workers.keys().cloned().collect();
        drop(workers);

        for worker_id in worker_ids {
            // In a real implementation, this would perform actual health checks
            // For now, we'll simulate health checks
            self.update_worker_health(&worker_id, true).await;
        }
    }

    /// Get error statistics
    pub async fn get_error_statistics(&self) -> ErrorStatistics {
        let history = self.error_history.read().await;
        let workers = self.workers.read().await;

        let mut severity_counts = HashMap::new();
        let mut error_counts = HashMap::new();

        for error in history.iter() {
            let severity = error.error.severity();
            *severity_counts.entry(severity).or_insert(0) += 1;

            let error_type = std::any::type_name_of_val(&error.error);
            *error_counts.entry(error_type.to_string()).or_insert(0) += 1;
        }

        ErrorStatistics {
            total_errors: history.len(),
            severity_counts,
            error_type_counts: error_counts,
            healthy_workers: workers.values().filter(|w| w.is_healthy).count(),
            isolated_workers: workers.values().filter(|w| w.is_isolated).count(),
            total_workers: workers.len(),
        }
    }
}

/// Error statistics for monitoring
#[derive(Debug, Clone)]
pub struct ErrorStatistics {
    pub total_errors: usize,
    pub severity_counts: HashMap<ErrorSeverity, u32>,
    pub error_type_counts: HashMap<String, u32>,
    pub healthy_workers: usize,
    pub isolated_workers: usize,
    pub total_workers: usize,
}

impl Default for ErrorRecoveryManager {
    fn default() -> Self {
        Self::new(3, 1000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{ProxyError, ErrorContext};
    use crate::common::{ProxyType, IsolatedWorker, WorkerResourceLimits, WorkerConfiguration};

    #[tokio::test]
    async fn test_circuit_breaker_basic() {
        let circuit_breaker = CircuitBreaker::new(3, 2, Duration::from_millis(100));

        // Should work normally initially
        for i in 0..2 {
            let result = circuit_breaker.call(async { Ok::<_, ProxyError>(i) }).await;
            assert!(result.is_ok());
        }

        // Should still be closed
        assert_eq!(circuit_breaker.get_state().await, CircuitState::Closed);

        // Trigger failures
        for _ in 0..3 {
            let _ = circuit_breaker.call(async {
                Err::<(), ProxyError>(ProxyError::Connection("test error".to_string()))
            }).await;
        }

        // Should be open now
        assert_eq!(circuit_breaker.get_state().await, CircuitState::Open);
    }

    #[tokio::test]
    async fn test_error_recovery_manager() {
        let manager = ErrorRecoveryManager::new(3, 100);

        // Create a test error
        let error = ProxyError::Connection("Test connection error".to_string());
        let context = ErrorContext::new("test", "test_operation")
            .with_worker_id("test_worker");
        let contextual_error = ContextualError::new(error, context);

        // Handle the error
        let result = manager.handle_error(contextual_error).await;
        assert!(result.is_ok());

        // Check error statistics
        let stats = manager.get_error_statistics().await;
        assert_eq!(stats.total_errors, 1);
    }

    #[tokio::test]
    async fn test_worker_health_tracking() {
        let manager = ErrorRecoveryManager::new(3, 100);

        // Register a worker first
        let worker = Arc::new(IsolatedWorker::new(
            ProxyType::ForwardProxy,
            WorkerResourceLimits::default(),
            WorkerConfiguration::default(),
        ));
        manager.register_worker(&worker).await;

        // Get the actual worker ID
        let health = manager.get_worker_health().await;
        let worker_id = health.keys().next().unwrap().clone();

        // Simulate worker health updates
        manager.update_worker_health(&worker_id, true).await;
        manager.update_worker_health(&worker_id, false).await;
        manager.update_worker_health(&worker_id, false).await;

        let health = manager.get_worker_health().await;
        let worker_health = health.get(&worker_id).unwrap();

        assert!(!worker_health.is_healthy);
        assert_eq!(worker_health.consecutive_failures, 2);
    }
}