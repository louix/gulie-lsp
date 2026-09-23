//! Supervisor tests against a real Guile. Skipped (with a note) when neither
//! `guile` nor `guix` is on PATH.

use guile_lsp::syntax::datum::Datum;
use guile_lsp::worker::protocol::WorkerError;
use guile_lsp::worker::{default_command, LogLevel, Logger, Trust, Worker, WorkerConfig};
use std::sync::{Arc, Mutex};
use std::time::Duration;

type Logs = Arc<Mutex<Vec<(LogLevel, String)>>>;

fn logs() -> (Logger, Logs) {
    let store = Arc::new(Mutex::new(Vec::new()));
    let sink = store.clone();
    (
        Arc::new(move |level, message| sink.lock().unwrap().push((level, message))),
        store,
    )
}

fn config(trust: Trust) -> WorkerConfig {
    WorkerConfig {
        command: Vec::new(),
        load_path: vec![concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/guile").into()],
        trust,
        deadline: Duration::from_secs(20),
        max_restarts: 1,
    }
}

fn guile_available() -> bool {
    if default_command().is_none() {
        eprintln!("no guile or guix on PATH; skipping worker test");
        return false;
    }
    true
}

fn params(text: &str) -> Datum {
    Datum::parse_one(text).unwrap()
}

#[tokio::test]
async fn lookup_exports_and_identity_in_a_trusted_workspace() {
    if !guile_available() {
        return;
    }
    let (logger, logs) = logs();
    let worker = Worker::new(config(Trust::Trusted), logger);
    let identity = worker.identity().await.unwrap();
    assert!(identity.guile.starts_with("3."), "{identity:?}");
    assert!(identity.key.starts_with("guile-3."));
    assert!(logs
        .lock()
        .unwrap()
        .iter()
        .any(|(_, m)| m.contains("Guile worker ready")));

    let fold = worker
        .request(
            "lookup",
            params("((name . fold) (modules (srfi srfi-1) (guile)))"),
        )
        .await
        .unwrap();
    assert_eq!(fold.assq("module"), Some(&params("(module srfi srfi-1)")));
    let binding = fold.assq("binding").unwrap();
    assert_eq!(binding.assq("kind"), Some(&Datum::symbol("procedure")));
    assert!(binding
        .assq("location")
        .unwrap()
        .assq("file")
        .unwrap()
        .as_str()
        .unwrap()
        .ends_with("srfi-1.scm"));

    let local = worker
        .request(
            "lookup",
            params("((name . connect) (modules (lambda-star)))"),
        )
        .await
        .unwrap();
    assert_eq!(
        local
            .assq("binding")
            .unwrap()
            .assq("documentation")
            .and_then(Datum::as_str),
        Some("Connect to HOST on PORT.  TIMEOUT is in seconds.")
    );

    let missing = worker
        .request(
            "lookup",
            params("((name . nope) (modules (srfi srfi-1) (no such module)))"),
        )
        .await;
    assert!(
        matches!(missing, Err(WorkerError::Remote { ref kind, .. }) if kind == "not-found"),
        "{missing:?}"
    );

    let exports = worker
        .request("exports", params("((module macros))"))
        .await
        .unwrap();
    assert_eq!(
        exports.assq("bindings").unwrap().as_list().unwrap().len() - 1,
        3
    );

    let unknown = worker.request("frobnicate", Datum::list([])).await;
    assert!(
        matches!(unknown, Err(WorkerError::Remote { ref kind, .. }) if kind == "unknown-method")
    );

    let failing = worker
        .request("exports", params("((module pathological fails-on-load))"))
        .await;
    assert!(
        matches!(failing, Err(WorkerError::Remote { ref kind, ref message }) if kind == "failed" && message.contains("fixture failure")),
        "{failing:?}"
    );
    // The worker survived the failing module.
    assert!(worker.request("hello", Datum::list([])).await.is_ok());
}

#[tokio::test]
async fn restricted_workspaces_only_get_source_only_requests() {
    if !guile_available() {
        return;
    }
    let (logger, _) = logs();
    let worker = Worker::new(config(Trust::Restricted), logger);
    let denied = worker
        .request("lookup", params("((name . fold) (modules (srfi srfi-1)))"))
        .await;
    assert!(
        matches!(denied, Err(WorkerError::Remote { ref kind, .. }) if kind == "restricted"),
        "{denied:?}"
    );
    let records = worker
        .request(
            "source-records",
            Datum::list([Datum::Pair(
                Box::new(Datum::symbol("file")),
                Box::new(Datum::string(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/fixtures/guile/records.scm"
                ))),
            )]),
        )
        .await
        .unwrap();
    assert_eq!(
        records.assq("records").unwrap().as_list().unwrap().len() - 1,
        2
    );
    assert_eq!(worker.identity().await.unwrap().trust, Trust::Restricted);
}

#[tokio::test]
async fn hangs_are_interrupted_and_dropped_requests_are_cancelled() {
    if !guile_available() {
        return;
    }
    let (logger, logs) = logs();
    let mut config = config(Trust::Trusted);
    config.deadline = Duration::from_millis(1500);
    let worker = Worker::new(config, logger);
    let pid_before = worker.identity().await.unwrap();

    let started = std::time::Instant::now();
    let hung = worker
        .request("exports", params("((module pathological hangs-on-load))"))
        .await;
    assert_eq!(hung, Err(WorkerError::Timeout));
    assert!(started.elapsed() < Duration::from_secs(5));
    // Interrupted, not killed: the same process keeps serving.
    assert!(worker.request("hello", Datum::list([])).await.is_ok());
    assert!(
        !logs
            .lock()
            .unwrap()
            .iter()
            .any(|(_, m)| m.contains("restart")),
        "{:?}",
        logs.lock().unwrap()
    );

    // Dropping the future (as tower-lsp does on $/cancelRequest) cancels too.
    // A fresh module is needed: the interrupted one is already registered.
    let dropped = tokio::time::timeout(
        Duration::from_millis(300),
        worker.request(
            "exports",
            params("((module pathological hangs-on-load-again))"),
        ),
    )
    .await;
    assert!(
        dropped.is_err(),
        "the request should still be running when dropped"
    );
    let started = std::time::Instant::now();
    assert!(worker.request("hello", Datum::list([])).await.is_ok());
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "cancelled request did not release the worker"
    );
    assert_eq!(worker.identity().await.unwrap(), pid_before);
}

#[tokio::test]
async fn crashes_are_reported_and_restarts_are_bounded() {
    if !guile_available() {
        return;
    }
    let (logger, logs) = logs();
    let worker = Worker::new(config(Trust::Trusted), logger);
    let hello = worker.request("hello", Datum::list([])).await.unwrap();
    let kill = |hello: &Datum| {
        let pid = hello.assq("pid").unwrap().as_integer().unwrap() as libc::pid_t;
        unsafe { libc::kill(pid, libc::SIGKILL) };
    };
    kill(&hello);
    std::thread::sleep(Duration::from_millis(200));
    let after_kill = worker.request("hello", Datum::list([])).await;
    // Either the write fails or the reply never comes: both are crashes.
    assert!(
        matches!(after_kill, Err(WorkerError::Crashed(_))),
        "{after_kill:?}"
    );
    assert!(logs
        .lock()
        .unwrap()
        .iter()
        .any(|(l, m)| *l == LogLevel::Warning && m.contains("will restart")));

    // One restart is allowed: the next request brings a fresh worker.
    let restarted = worker.request("hello", Datum::list([])).await.unwrap();
    assert_ne!(restarted.assq("pid"), hello.assq("pid"));
    kill(&restarted);
    std::thread::sleep(Duration::from_millis(200));
    assert!(matches!(
        worker.request("hello", Datum::list([])).await,
        Err(WorkerError::Crashed(_))
    ));
    let disabled = worker.request("hello", Datum::list([])).await;
    assert!(
        matches!(disabled, Err(WorkerError::Unavailable(ref why)) if why.contains("giving up")),
        "{disabled:?}"
    );
    assert!(logs
        .lock()
        .unwrap()
        .iter()
        .any(|(l, _)| *l == LogLevel::Error));
}

#[tokio::test]
async fn missing_environment_is_an_actionable_error() {
    let (logger, logs) = logs();
    let mut config = config(Trust::Trusted);
    config.command = vec!["/nonexistent/guile".into()];
    let worker = Worker::new(config, logger);
    let err = worker.identity().await.unwrap_err();
    assert!(
        matches!(err, WorkerError::Unavailable(ref why) if why.contains("cannot start Guile worker")),
        "{err:?}"
    );
    assert_eq!(logs.lock().unwrap().len(), 1);
    // No retries for a configuration problem.
    assert!(matches!(
        worker.request("hello", Datum::list([])).await,
        Err(WorkerError::Unavailable(_))
    ));
}

#[tokio::test]
async fn shutdown_does_not_wait_for_a_stuck_request() {
    if !guile_available() {
        return;
    }
    let (logger, _logs) = logs();
    let mut config = config(Trust::Trusted);
    config.deadline = Duration::from_secs(60);
    let worker = Worker::new(config, logger);
    worker.identity().await.unwrap();
    let stuck = worker.clone();
    let request = tokio::spawn(async move {
        stuck
            .request("exports", params("((module pathological hangs-on-load))"))
            .await
    });
    tokio::time::sleep(Duration::from_millis(500)).await;
    let started = std::time::Instant::now();
    worker.shutdown().await;
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "shutdown took {:?}",
        started.elapsed()
    );
    assert!(request.await.unwrap().is_err());
}

#[tokio::test]
async fn loaded_modules_are_searched_for_a_name_s_exporters() {
    if !guile_available() {
        return;
    }
    let (logger, _logs) = logs();
    let worker = Worker::new(config(Trust::Trusted), logger);
    // Listing exports loads the module; the search itself only reads what is loaded.
    worker
        .request("exports", params("((module srfi srfi-1))"))
        .await
        .unwrap();
    let found = worker
        .request("find-exporters", params("((name . \"fold\"))"))
        .await
        .unwrap();
    let modules = found.assq("modules").unwrap().to_string();
    assert!(modules.contains("(srfi srfi-1)"), "{modules}");
    let none = worker
        .request(
            "find-exporters",
            params("((name . \"no-such-name-anywhere\"))"),
        )
        .await
        .unwrap();
    // An empty list prints as the bare entry.
    assert_eq!(none.to_string(), "((modules))");
}
