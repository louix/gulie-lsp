//! Server settings from `initializationOptions`, documented in the README.

use crate::worker::{Trust, WorkerConfig};
use serde_json::Value;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub worker: WorkerConfig,
    pub problems: Vec<String>,
    /// Extra indentation rules: head symbol to distinguished-argument count.
    pub indent: Vec<(String, u8)>,
    /// How long one explicit validation run may take.
    pub validate_timeout: Duration,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            worker: WorkerConfig::default(),
            problems: Vec::new(),
            indent: Vec::new(),
            validate_timeout: Duration::from_secs(120),
        }
    }
}

impl PartialEq for WorkerConfig {
    fn eq(&self, other: &Self) -> bool {
        self.command == other.command
            && self.load_path == other.load_path
            && self.trust == other.trust
            && self.deadline == other.deadline
            && self.max_restarts == other.max_restarts
    }
}

impl Settings {
    pub fn from_initialization_options(options: Option<&Value>) -> Settings {
        let mut settings = Settings::default();
        let Some(guile) = options.and_then(|o| o.get("guile")) else {
            return settings;
        };
        let mut problem = |text: String| settings.problems.push(text);
        match guile.get("command") {
            None | Some(Value::Null) => {}
            Some(Value::Array(items))
                if items.iter().all(Value::is_string) && !items.is_empty() =>
            {
                settings.worker.command = items
                    .iter()
                    .map(|v| v.as_str().unwrap().to_string())
                    .collect();
            }
            Some(other) => problem(format!(
                "guile.command must be a non-empty array of strings, got {other}"
            )),
        }
        match guile.get("loadPath") {
            None | Some(Value::Null) => {}
            Some(Value::Array(items)) if items.iter().all(Value::is_string) => {
                settings.worker.load_path = items
                    .iter()
                    .map(|v| v.as_str().unwrap().to_string())
                    .collect();
            }
            Some(other) => problem(format!(
                "guile.loadPath must be an array of strings, got {other}"
            )),
        }
        match guile.get("trust").and_then(Value::as_str) {
            None => {}
            Some("trusted") => settings.worker.trust = Trust::Trusted,
            Some("restricted") => settings.worker.trust = Trust::Restricted,
            Some(other) => problem(format!(
                "guile.trust must be \"trusted\" or \"restricted\", got {other:?}"
            )),
        }
        match guile.get("deadlineMs") {
            None | Some(Value::Null) => {}
            Some(value) => match value.as_u64() {
                Some(ms) if ms > 0 => settings.worker.deadline = Duration::from_millis(ms),
                _ => problem(format!(
                    "guile.deadlineMs must be a positive integer, got {value}"
                )),
            },
        }
        match guile.get("validateTimeoutMs") {
            None | Some(Value::Null) => {}
            Some(value) => match value.as_u64() {
                Some(ms) if ms > 0 => settings.validate_timeout = Duration::from_millis(ms),
                _ => problem(format!(
                    "guile.validateTimeoutMs must be a positive integer, got {value}"
                )),
            },
        }
        match guile.get("indent") {
            None | Some(Value::Null) => {}
            Some(Value::Object(entries)) => {
                for (name, count) in entries {
                    match count.as_u64().and_then(|n| u8::try_from(n).ok()) {
                        Some(n) => settings.indent.push((name.clone(), n)),
                        None => problem(format!(
                            "guile.indent.{name} must be a small non-negative integer, got {count}"
                        )),
                    }
                }
            }
            Some(other) => problem(format!(
                "guile.indent must be an object of name to argument count, got {other}"
            )),
        }
        settings
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_options_and_reports_problems_without_failing() {
        let options = json!({ "guile": {
            "command": ["guix", "repl", "--"], "loadPath": ["."],
            "trust": "trusted", "deadlineMs": 500, "indent": { "my-let": 1 }, "validateTimeoutMs": 7000 } });
        let settings = Settings::from_initialization_options(Some(&options));
        assert_eq!(settings.worker.command, vec!["guix", "repl", "--"]);
        assert_eq!(settings.worker.load_path, vec!["."]);
        assert_eq!(settings.worker.trust, Trust::Trusted);
        assert_eq!(settings.worker.deadline, Duration::from_millis(500));
        assert_eq!(settings.indent, vec![("my-let".to_string(), 1)]);
        assert_eq!(settings.validate_timeout, Duration::from_millis(7000));
        assert!(settings.problems.is_empty());

        let bad = json!({ "guile": { "command": "guile", "trust": "yes", "deadlineMs": 0, "indent": { "x": -1 } } });
        let settings = Settings::from_initialization_options(Some(&bad));
        assert_eq!(settings.problems.len(), 4, "{:?}", settings.problems);
        assert_eq!(settings.worker, WorkerConfig::default());
        assert_eq!(
            Settings::from_initialization_options(None),
            Settings::default()
        );
    }
}
