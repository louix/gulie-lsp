//! Framing and message shapes of the worker protocol (version 1).
//! See `guile/guile-lsp/worker.scm` for the authoritative description.

use crate::syntax::datum::Datum;
use std::fmt;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

pub const PROTOCOL_VERSION: i64 = 1;

/// Encode one datum with `Content-Length` framing.
pub fn frame(datum: &Datum) -> Vec<u8> {
    let body = datum.to_string();
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend_from_slice(body.as_bytes());
    out
}

/// Read one framed datum. `Ok(None)` at end of stream.
pub async fn read_frame<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut BufReader<R>,
) -> std::io::Result<Option<Datum>> {
    let mut length = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).await? == 0 {
            return Ok(None);
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            if length.is_some() {
                break;
            }
            continue;
        }
        if let Some(value) = line
            .get(..15)
            .filter(|h| h.eq_ignore_ascii_case("content-length:"))
            .map(|_| &line[15..])
        {
            length = value.trim().parse::<usize>().ok();
        }
    }
    let mut body = vec![0; length.unwrap_or(0)];
    reader.read_exact(&mut body).await?;
    let text = String::from_utf8(body)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    Datum::parse_one(&text).map(Some).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("unreadable worker frame: {text}"),
        )
    })
}

pub async fn write_frame<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut W,
    datum: &Datum,
) -> std::io::Result<()> {
    writer.write_all(&frame(datum)).await?;
    writer.flush().await
}

pub fn request(id: i64, method: &str, params: Datum) -> Datum {
    Datum::list([
        Datum::symbol("request"),
        Datum::Integer(id),
        Datum::symbol(method),
        params,
    ])
}

pub fn cancel(id: i64) -> Datum {
    Datum::list([Datum::symbol("cancel"), Datum::Integer(id)])
}

pub fn shutdown() -> Datum {
    Datum::list([Datum::symbol("shutdown")])
}

/// A message from the worker.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    Ok {
        id: i64,
        result: Datum,
    },
    Error {
        id: i64,
        kind: String,
        message: String,
    },
    Log {
        level: String,
        message: String,
    },
}

impl Incoming {
    pub fn parse(datum: &Datum) -> Option<Incoming> {
        let items = datum.as_list()?;
        match items {
            [tag, id, status, rest @ ..] if tag.as_symbol() == Some("response") => {
                let id = id.as_integer()?;
                match (status.as_symbol()?, rest) {
                    ("ok", [result]) => Some(Incoming::Ok {
                        id,
                        result: result.clone(),
                    }),
                    ("error", [kind, message]) => Some(Incoming::Error {
                        id,
                        kind: kind.as_symbol()?.to_string(),
                        message: message.as_str()?.to_string(),
                    }),
                    _ => None,
                }
            }
            [tag, level, message] if tag.as_symbol() == Some("log") => Some(Incoming::Log {
                level: level.as_symbol()?.to_string(),
                message: message.as_str()?.to_string(),
            }),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum WorkerError {
    /// The worker is not running and will not be restarted.
    Unavailable(String),
    /// The request exceeded its deadline; the worker was interrupted or restarted.
    Timeout,
    /// The caller dropped the request.
    Cancelled,
    /// The worker answered with a structured failure.
    Remote { kind: String, message: String },
    /// The worker died while the request was in flight.
    Crashed(String),
}

impl fmt::Display for WorkerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WorkerError::Unavailable(why) => write!(f, "Guile worker unavailable: {why}"),
            WorkerError::Timeout => f.write_str("Guile worker request timed out"),
            WorkerError::Cancelled => f.write_str("request cancelled"),
            WorkerError::Remote { kind, message } => write!(f, "{kind}: {message}"),
            WorkerError::Crashed(why) => write!(f, "Guile worker crashed: {why}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn frames_round_trip_with_multibyte_bodies() {
        let datum = request(
            7,
            "lookup",
            Datum::parse_one("((name . λ🦀) (modules (guile)))").unwrap(),
        );
        let bytes = frame(&datum);
        assert!(bytes.starts_with(b"Content-Length: "));
        let mut reader = BufReader::new(std::io::Cursor::new([bytes.clone(), bytes].concat()));
        assert_eq!(read_frame(&mut reader).await.unwrap(), Some(datum.clone()));
        assert_eq!(read_frame(&mut reader).await.unwrap(), Some(datum));
        assert_eq!(read_frame(&mut reader).await.unwrap(), None);
    }

    #[test]
    fn incoming_messages_parse() {
        let ok = Datum::parse_one("(response 3 ok ((guile . \"3.0.10\")))").unwrap();
        assert!(matches!(
            Incoming::parse(&ok),
            Some(Incoming::Ok { id: 3, .. })
        ));
        let err = Datum::parse_one("(response 4 error restricted \"needs trust\")").unwrap();
        assert_eq!(
            Incoming::parse(&err),
            Some(Incoming::Error {
                id: 4,
                kind: "restricted".into(),
                message: "needs trust".into()
            })
        );
        let log = Datum::parse_one("(log warning \"ignoring\")").unwrap();
        assert!(matches!(Incoming::parse(&log), Some(Incoming::Log { .. })));
        assert_eq!(Incoming::parse(&Datum::parse_one("(bogus)").unwrap()), None);
    }
}
