use crate::config::{LoggingConfig, LogLevel, LogFormat, LogTarget, LogOutputType};
use log::Record;
use std::io::{Write, BufWriter};
use std::fs::OpenOptions;
use std::sync::Mutex;
use chrono::{DateTime, Utc};
use serde_json::json;

pub struct CustomLogger {
    targets: Vec<LogTarget>,
    format: LogFormat,
    writers: Vec<Mutex<BufWriter<Box<dyn Write + Send>>>>,
}

impl CustomLogger {
    pub fn new(config: LoggingConfig) -> Result<Self, Box<dyn std::error::Error>> {
        let format = config.format.unwrap_or_default();
        let targets = config.targets.unwrap_or_default();

        let mut writers = Vec::new();

        for target in &targets {
            let writer: Box<dyn Write + Send> = match target.output_type {
                LogOutputType::Stdout => Box::new(std::io::stdout()),
                LogOutputType::File => {
                    let path = target.path.as_ref()
                        .ok_or("File output type requires path")?;
                    let file = OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(path)?;
                    Box::new(file)
                }
            };
            writers.push(Mutex::new(BufWriter::new(writer)));
        }

        Ok(Self {
            targets,
            format,
            writers,
        })
    }

    pub fn init(config: LoggingConfig) -> Result<(), Box<dyn std::error::Error>> {
        let logger = Self::new(config)?;
        log::set_boxed_logger(Box::new(logger))?;

        // Note: Individual targets handle their own filtering, so we allow all levels here

        Ok(())
    }

    fn should_log(&self, record: &Record, target: &LogTarget) -> bool {
        if let Some(target_level) = &target.level {
            match (record.level(), target_level) {
                (log::Level::Trace, LogLevel::Trace) => true,
                (log::Level::Trace, LogLevel::Debug) => true,
                (log::Level::Trace, LogLevel::Info) => true,
                (log::Level::Trace, LogLevel::Warn) => true,
                (log::Level::Trace, LogLevel::Error) => true,

                (log::Level::Debug, LogLevel::Trace) => false,
                (log::Level::Debug, LogLevel::Debug) => true,
                (log::Level::Debug, LogLevel::Info) => true,
                (log::Level::Debug, LogLevel::Warn) => true,
                (log::Level::Debug, LogLevel::Error) => true,

                (log::Level::Info, LogLevel::Trace) => false,
                (log::Level::Info, LogLevel::Debug) => false,
                (log::Level::Info, LogLevel::Info) => true,
                (log::Level::Info, LogLevel::Warn) => true,
                (log::Level::Info, LogLevel::Error) => true,

                (log::Level::Warn, LogLevel::Trace) => false,
                (log::Level::Warn, LogLevel::Debug) => false,
                (log::Level::Warn, LogLevel::Info) => false,
                (log::Level::Warn, LogLevel::Warn) => true,
                (log::Level::Warn, LogLevel::Error) => true,

                (log::Level::Error, LogLevel::Trace) => false,
                (log::Level::Error, LogLevel::Debug) => false,
                (log::Level::Error, LogLevel::Info) => false,
                (log::Level::Error, LogLevel::Warn) => false,
                (log::Level::Error, LogLevel::Error) => true,
            }
        } else {
            true // If no target level specified, log all levels
        }
    }

    fn format_text(&self, record: &Record) -> String {
        let timestamp: DateTime<Utc> = Utc::now();
        format!(
            "{} [{}] [{}] [{}:{}] {}",
            timestamp.format("%Y-%m-%d %H:%M:%S%.3f"),
            record.level().to_string().to_uppercase(),
            record.target(),
            record.file().unwrap_or("unknown"),
            record.line().unwrap_or(0),
            record.args()
        )
    }

    fn format_json(&self, record: &Record) -> String {
        let timestamp: DateTime<Utc> = Utc::now();
        let timestamp_str = timestamp.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
        let log_entry = json!({
            "timestamp": timestamp_str,
            "level": record.level().to_string().to_lowercase(),
            "target": record.target(),
            "module": record.module_path().unwrap_or("unknown"),
            "file": record.file().unwrap_or("unknown"),
            "line": record.line().unwrap_or(0),
            "message": record.args().to_string(),
        });

        serde_json::to_string(&log_entry).unwrap_or_else(|_| {
            json!({"error": "Failed to serialize log entry", "raw_message": record.args().to_string()})
                .to_string()
        })
    }
}

impl log::Log for CustomLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        // Check if any target should log this level
        self.targets.iter().any(|target| {
            if let Some(target_level) = &target.level {
                let record_level = metadata.level();
                match (record_level, target_level) {
                    (log::Level::Trace, LogLevel::Trace) => true,
                    (log::Level::Trace, LogLevel::Debug) => true,
                    (log::Level::Trace, LogLevel::Info) => true,
                    (log::Level::Trace, LogLevel::Warn) => true,
                    (log::Level::Trace, LogLevel::Error) => true,
                    (log::Level::Debug, LogLevel::Trace) => false,
                    (log::Level::Debug, LogLevel::Debug) => true,
                    (log::Level::Debug, LogLevel::Info) => true,
                    (log::Level::Debug, LogLevel::Warn) => true,
                    (log::Level::Debug, LogLevel::Error) => true,
                    (log::Level::Info, LogLevel::Trace) => false,
                    (log::Level::Info, LogLevel::Debug) => false,
                    (log::Level::Info, LogLevel::Info) => true,
                    (log::Level::Info, LogLevel::Warn) => true,
                    (log::Level::Info, LogLevel::Error) => true,
                    (log::Level::Warn, LogLevel::Trace) => false,
                    (log::Level::Warn, LogLevel::Debug) => false,
                    (log::Level::Warn, LogLevel::Info) => false,
                    (log::Level::Warn, LogLevel::Warn) => true,
                    (log::Level::Warn, LogLevel::Error) => true,
                    (log::Level::Error, LogLevel::Trace) => false,
                    (log::Level::Error, LogLevel::Debug) => false,
                    (log::Level::Error, LogLevel::Info) => false,
                    (log::Level::Error, LogLevel::Warn) => false,
                    (log::Level::Error, LogLevel::Error) => true,
                }
            } else {
                true // No level restriction
            }
        })
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }

        let message = match self.format {
            LogFormat::Text => self.format_text(record),
            LogFormat::Json => self.format_json(record),
        };

        for (i, target) in self.targets.iter().enumerate() {
            if self.should_log(record, target) {
                if let Ok(mut writer) = self.writers[i].lock() {
                    let _ = writeln!(writer, "{}", message);
                    let _ = writer.flush();
                }
            }
        }
    }

    fn flush(&self) {
        for writer in &self.writers {
            if let Ok(mut w) = writer.lock() {
                let _ = w.flush();
            }
        }
    }
}

// Fallback to env_logger if custom logging configuration is not provided
pub fn init_fallback(log_level: Option<&str>, log_format: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    let level = log_level.unwrap_or("info");
    let format = log_format.unwrap_or("text");

    if format == "json" {
        // For JSON format with env_logger, we need a different approach
        // For now, we'll use a simple JSON formatter with env_logger
        let mut builder = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(level));
        builder.format(|buf, record| {
            let timestamp: DateTime<Utc> = Utc::now();
            let timestamp_str = timestamp.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
            let log_entry = json!({
                "timestamp": timestamp_str,
                "level": record.level().to_string().to_lowercase(),
                "target": record.target(),
                "module": record.module_path().unwrap_or("unknown"),
                "file": record.file().unwrap_or("unknown"),
                "line": record.line().unwrap_or(0),
                "message": record.args().to_string(),
            });
            writeln!(buf, "{}", serde_json::to_string(&log_entry).unwrap())
        });
        builder.init();
    } else {
        // Use standard env_logger for text format
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(level)).init();
    }

    Ok(())
}

// Parse string to LogLevel
pub fn parse_log_level(s: &str) -> Result<LogLevel, Box<dyn std::error::Error>> {
    match s.to_lowercase().as_str() {
        "trace" => Ok(LogLevel::Trace),
        "debug" => Ok(LogLevel::Debug),
        "info" => Ok(LogLevel::Info),
        "warn" => Ok(LogLevel::Warn),
        "error" => Ok(LogLevel::Error),
        _ => Err(format!("Invalid log level: {}. Must be one of: trace, debug, info, warn, error", s).into()),
    }
}

// Parse string to LogFormat
pub fn parse_log_format(s: &str) -> Result<LogFormat, Box<dyn std::error::Error>> {
    match s.to_lowercase().as_str() {
        "text" => Ok(LogFormat::Text),
        "json" => Ok(LogFormat::Json),
        _ => Err(format!("Invalid log format: {}. Must be one of: text, json", s).into()),
    }
}