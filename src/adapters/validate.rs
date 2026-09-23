//! Explicit Guix configuration validation: load a saved file the way
//! `guix system` and `guix home` do, in a separate short-lived process, and
//! report what happened. Nothing is built; see `guile/guile-lsp/validate.scm`
//! for exactly what is checked.

use crate::syntax::datum::Datum;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

/// Captured output is cut off here; a runaway `display` loop must not
/// exhaust memory.
const OUTPUT_LIMIT: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    pub message: String,
    pub file: Option<String>,
    /// 1-based line and 0-based column, as Guix reports them.
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Validation {
    /// `operating-system`, `home-environment`, `package`, `manifest`,
    /// `unknown`, or `error`.
    pub kind: String,
    pub summary: String,
    /// What the run actually checked, for the report.
    pub checks: String,
    pub errors: Vec<ValidationError>,
}

impl Validation {
    pub fn ok(&self) -> bool {
        self.errors.is_empty() && self.kind != "error"
    }
}

#[derive(Debug)]
pub enum Failure {
    Spawn(std::io::Error),
    TimedOut(Duration),
    /// The process ended without a parsable report; carries what it printed.
    NoReport {
        status: String,
        output: String,
    },
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failure::Spawn(e) => write!(f, "cannot start the validation process: {e}"),
            Failure::TimedOut(limit) => {
                write!(
                    f,
                    "validation exceeded {} s and was stopped",
                    limit.as_secs()
                )
            }
            Failure::NoReport { status, output } => {
                write!(f, "validation process {status} without a report")?;
                let trimmed = output.trim();
                if !trimmed.is_empty() {
                    write!(f, ": {trimmed}")?;
                }
                Ok(())
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    /// The worker command, e.g. `["guix", "repl", "--"]`; `-L` arguments go
    /// before a trailing `--` as for the worker.
    pub command: Vec<String>,
    pub load_path: Vec<String>,
    pub timeout: Duration,
    /// Directory holding `validate.scm` (see `worker::install_sources`).
    pub script_dir: PathBuf,
}

/// Run the validation script on `file`, with the file's directory as the
/// working directory so relative references resolve as they do for
/// `guix system`. Dropping the returned future kills the process.
pub async fn validate(config: &Config, file: &Path) -> Result<Validation, Failure> {
    let mut args: Vec<String> = config.command[1..].to_vec();
    let mut load_args = Vec::new();
    for path in &config.load_path {
        load_args.push("-L".to_string());
        load_args.push(path.clone());
    }
    match args.iter().position(|a| a == "--") {
        Some(dashes) => {
            let tail = args.split_off(dashes);
            args.extend(load_args);
            args.extend(tail);
        }
        None => args.extend(load_args),
    }
    args.push(
        config
            .script_dir
            .join("validate.scm")
            .to_string_lossy()
            .into_owned(),
    );
    args.push(file.to_string_lossy().into_owned());
    let mut child = Command::new(&config.command[0])
        .args(&args)
        .current_dir(file.parent().unwrap_or(Path::new(".")))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(Failure::Spawn)?;
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let run = async {
        let (out, err) = tokio::join!(read_limited(&mut stdout), read_limited(&mut stderr));
        let status = child.wait().await;
        (out, err, status)
    };
    let (out, err, status) = match tokio::time::timeout(config.timeout, run).await {
        Ok(result) => result,
        Err(_) => {
            let _ = child.kill().await;
            return Err(Failure::TimedOut(config.timeout));
        }
    };
    match parse_report(&out) {
        Some(validation) => Ok(validation),
        None => Err(Failure::NoReport {
            status: match status {
                Ok(s) if s.success() => "exited successfully".to_string(),
                Ok(s) => format!("exited with {s}"),
                Err(e) => format!("could not be waited for ({e})"),
            },
            output: [err.trim(), out.trim()]
                .iter()
                .filter(|s| !s.is_empty())
                .cloned()
                .collect::<Vec<_>>()
                .join("\n"),
        }),
    }
}

async fn read_limited(reader: &mut (impl AsyncReadExt + Unpin)) -> String {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if buffer.len() < OUTPUT_LIMIT {
                    buffer.extend_from_slice(&chunk[..n]);
                }
                // Keep draining so the child never blocks on a full pipe.
            }
        }
    }
    buffer.truncate(OUTPUT_LIMIT);
    String::from_utf8_lossy(&buffer).into_owned()
}

/// The last `(validation ...)` datum in `output`; user code may print
/// before it.
pub fn parse_report(output: &str) -> Option<Validation> {
    let start = output.rfind("(validation ")?;
    let datum = Datum::parse_one(output[start..].trim())?;
    let field = |name: &str| {
        datum.assq(name).and_then(|d| {
            d.as_str()
                .map(str::to_string)
                .or_else(|| d.as_symbol().map(str::to_string))
        })
    };
    let errors = datum
        .assq("errors")?
        .as_list()?
        .iter()
        .filter_map(|entry| {
            let text = |key: &str| entry.assq(key).and_then(Datum::as_str).map(str::to_string);
            let number = |key: &str| {
                entry
                    .assq(key)
                    .and_then(Datum::as_integer)
                    .and_then(|n| u32::try_from(n).ok())
            };
            Some(ValidationError {
                message: text("message")?,
                file: text("file"),
                line: number("line"),
                column: number("column"),
                hint: text("hint"),
            })
        })
        .collect();
    Some(Validation {
        kind: field("kind")?,
        summary: field("summary").unwrap_or_default(),
        checks: field("checks").unwrap_or_default(),
        errors,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_reports_after_stray_output() {
        let output = "hi(validation (file \"/x.scm\") (kind error) (summary \"failed to load\") (checks \"\") (errors (((message . \"Unbound variable: foo\") (file . \"/x.scm\") (line . 64) (column . 19) (hint . \"Did you forget a use-modules form?\")) ((message . \"other\")))))\n";
        let report = parse_report(output).unwrap();
        assert_eq!(report.kind, "error");
        assert!(!report.ok());
        assert_eq!(report.errors.len(), 2);
        assert_eq!(report.errors[0].line, Some(64));
        assert_eq!(
            report.errors[0].hint.as_deref(),
            Some("Did you forget a use-modules form?")
        );
        assert_eq!(report.errors[1].file, None);
        let clean = parse_report("(validation (file \"/x.scm\") (kind package) (summary \"package a@1\") (checks \"loaded\") (errors ()))").unwrap();
        assert!(clean.ok());
        assert_eq!(clean.summary, "package a@1");
        assert!(parse_report("nothing here").is_none());
    }
}
