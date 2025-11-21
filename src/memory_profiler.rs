//! Memory profiler for worker separation architecture
//!
//! This module provides tools to analyze memory usage patterns
//! and detect potential memory leaks or excessive usage in the
//! worker separation implementation.

use crate::common::IsolatedWorker;
use std::sync::Arc;

/// Memory usage statistics for workers
#[derive(Debug, Clone)]
pub struct MemoryStats {
    pub worker_count: usize,
    pub total_connections: u64,
    pub total_requests: u64,
    pub total_memory_estimate_mb: f64,
    pub metrics_memory_estimate_mb: f64,
    pub connection_pool_memory_estimate_mb: f64,
}

impl MemoryStats {
    pub fn new() -> Self {
        Self {
            worker_count: 0,
            total_connections: 0,
            total_requests: 0,
            total_memory_estimate_mb: 0.0,
            metrics_memory_estimate_mb: 0.0,
            connection_pool_memory_estimate_mb: 0.0,
        }
    }

    pub fn total_memory_mb(&self) -> f64 {
        self.total_memory_estimate_mb
    }
}

/// Memory profiler for worker separation
pub struct MemoryProfiler {
    workers: Vec<Arc<IsolatedWorker>>,
    baseline_stats: MemoryStats,
}

impl MemoryProfiler {
    pub fn new() -> Self {
        Self {
            workers: Vec::new(),
            baseline_stats: Self::measure_system_baseline(),
        }
    }

    /// Add a worker to track
    pub fn add_worker(&mut self, worker: Arc<IsolatedWorker>) {
        self.workers.push(worker);
    }

    /// Measure system baseline memory usage
    fn measure_system_baseline() -> MemoryStats {
        // This is a simplified implementation
        // In a real scenario, you might use system APIs to get actual memory usage
        MemoryStats::new()
    }

    /// Calculate current memory usage
    pub fn calculate_memory_usage(&self) -> MemoryStats {
        let mut stats = MemoryStats::new();

        stats.worker_count = self.workers.len();

        for worker in &self.workers {
            // Count connections and requests
            stats.total_connections += worker.metrics.connections_active();
            stats.total_requests += worker.metrics.requests_total();

            // Estimate memory usage for each component
            stats.metrics_memory_estimate_mb += Self::estimate_metrics_memory();
            stats.connection_pool_memory_estimate_mb += Self::estimate_connection_pool_memory();
        }

        // Estimate total memory (simplified calculation)
        stats.total_memory_estimate_mb =
            stats.metrics_memory_estimate_mb +
            stats.connection_pool_memory_estimate_mb +
            (self.workers.len() as f64 * 2.0); // ~2MB per worker base overhead

        stats
    }

    /// Estimate memory usage for metrics
    fn estimate_metrics_memory() -> f64 {
        // Atomic counters: ~8 bytes each
        // String allocations and other overhead
        0.1 // 100KB estimated for metrics per worker
    }

    /// Estimate memory usage for connection pools
    fn estimate_connection_pool_memory() -> f64 {
        // Connection tracking and pool management overhead
        0.05 // 50KB estimated per worker
    }

    /// Detect potential memory leaks
    pub fn detect_memory_leaks(&self, threshold_mb: f64) -> Vec<String> {
        let mut warnings = Vec::new();
        let current_stats = self.calculate_memory_usage();

        let memory_increase = current_stats.total_memory_mb() - self.baseline_stats.total_memory_mb();

        if memory_increase > threshold_mb {
            warnings.push(format!(
                "Memory usage increased by {:.1} MB (threshold: {:.1} MB)",
                memory_increase, threshold_mb
            ));
        }

        // Check for excessive connections that might indicate leaks
        if current_stats.total_connections > 10000 {
            warnings.push(format!(
                "High connection count: {} (potential connection leak)",
                current_stats.total_connections
            ));
        }

        // Check memory per worker ratio
        if self.workers.len() > 0 {
            let memory_per_worker = current_stats.total_memory_mb() / self.workers.len() as f64;
            if memory_per_worker > 10.0 {
                warnings.push(format!(
                    "High memory per worker: {:.1} MB (threshold: 10.0 MB)",
                    memory_per_worker
                ));
            }
        }

        warnings
    }

    /// Generate memory usage report
    pub fn generate_report(&self) -> String {
        let stats = self.calculate_memory_usage();
        let warnings = self.detect_memory_leaks(50.0); // 50MB threshold

        let mut report = String::new();
        report.push_str("# Worker Separation Memory Usage Report\n\n");

        report.push_str("## Overview\n");
        report.push_str(&format!("- Worker Count: {}\n", stats.worker_count));
        report.push_str(&format!("- Total Connections: {}\n", stats.total_connections));
        report.push_str(&format!("- Total Requests: {}\n", stats.total_requests));
        report.push_str(&format!("- Estimated Memory Usage: {:.1} MB\n", stats.total_memory_mb()));
        report.push_str(&format!("- Memory per Worker: {:.1} MB\n\n",
            if stats.worker_count > 0 { stats.total_memory_mb() / stats.worker_count as f64 } else { 0.0 }));

        report.push_str("## Memory Breakdown\n");
        report.push_str(&format!("- Metrics Memory: {:.1} MB\n", stats.metrics_memory_estimate_mb));
        report.push_str(&format!("- Connection Pool Memory: {:.1} MB\n", stats.connection_pool_memory_estimate_mb));
        report.push_str(&format!("- Worker Base Overhead: {:.1} MB\n\n",
            stats.total_memory_mb() - stats.metrics_memory_estimate_mb - stats.connection_pool_memory_estimate_mb));

        if !warnings.is_empty() {
            report.push_str("## Warnings\n");
            for warning in warnings {
                report.push_str(&format!("⚠️  {}\n", warning));
            }
        } else {
            report.push_str("## Warnings\n✅ No memory usage warnings detected\n");
        }

        report.push_str("\n---\nGenerated by Bifrost Bridge Memory Profiler");

        report
    }

    /// Reset baseline to current state
    pub fn reset_baseline(&mut self) {
        self.baseline_stats = self.calculate_memory_usage();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::{WorkerResourceLimits, WorkerConfiguration, ProxyType};

    #[test]
    fn test_memory_profiler_creation() {
        let profiler = MemoryProfiler::new();
        assert_eq!(profiler.workers.len(), 0);
    }

    #[test]
    fn test_memory_profiler_add_worker() {
        let mut profiler = MemoryProfiler::new();
        let worker = Arc::new(IsolatedWorker::new(
            ProxyType::ForwardProxy,
            WorkerResourceLimits::default(),
            WorkerConfiguration::default(),
        ));

        profiler.add_worker(worker.clone());
        assert_eq!(profiler.workers.len(), 1);

        let stats = profiler.calculate_memory_usage();
        assert_eq!(stats.worker_count, 1);
    }

    #[test]
    fn test_memory_leak_detection() {
        let mut profiler = MemoryProfiler::new();

        // Add workers with high connection counts
        for _ in 0..10 {
            let worker = Arc::new(IsolatedWorker::new(
                ProxyType::ForwardProxy,
                WorkerResourceLimits::default(),
                WorkerConfiguration::default(),
            ));

            // Simulate high connection count
            for _ in 0..1001 {
                worker.increment_connections();
            }

            profiler.add_worker(worker);
        }

        let warnings = profiler.detect_memory_leaks(1.0); // Low threshold
        assert!(!warnings.is_empty());
        assert!(warnings.iter().any(|w| w.contains("High connection count")));
    }

    #[test]
    fn test_report_generation() {
        let mut profiler = MemoryProfiler::new();

        let worker = Arc::new(IsolatedWorker::new(
            ProxyType::ReverseProxy,
            WorkerResourceLimits::default(),
            WorkerConfiguration::default(),
        ));

        worker.increment_connections();
        worker.metrics.increment_requests_by(100);

        profiler.add_worker(worker);

        let report = profiler.generate_report();
        assert!(report.contains("Worker Separation Memory Usage Report"));
        assert!(report.contains("Worker Count: 1"));
        assert!(report.contains("Total Connections: 1"));
        assert!(report.contains("Total Requests: 100"));
    }
}
