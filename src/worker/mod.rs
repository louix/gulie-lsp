//! Supervisor for the Guile worker process.
//!
//! One worker runs at a time and handles requests sequentially. The
//! supervisor adds deadlines, cancellation (a dropped request interrupts the
//! worker with SIGUSR1), bounded restarts, and an environment identity from
//! the worker's `hello` reply.

pub mod protocol;

use crate::syntax::datum::Datum;
use protocol::{Incoming, WorkerError};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tokio::io::BufReader;
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{oneshot, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trust {
    Restricted,
    Trusted,
}

impl Trust {
    fn as_str(self) -> &'static str {
        match self {
            Trust::Restricted => "restricted",
            Trust::Trusted => "trusted",
        }
    }
}

#[derive(Debug, Clone)]
pub struct WorkerConfig {
    /// Program and leading arguments, e.g. `["guile"]` or `["guix", "repl", "--"]`.
    /// `-L` load-path arguments are inserted before a trailing `--`.
    pub command: Vec<String>,
    pub load_path: Vec<String>,
    pub trust: Trust,
    pub deadline: Duration,
    pub max_restarts: u32,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        WorkerConfig {
            command: Vec::new(),
            load_path: Vec::new(),
            trust: Trust::Restricted,
            deadline: Duration::from_secs(15),
            max_restarts: 3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Warning,
    Error,
}

pub type Logger = Arc<dyn Fn(LogLevel, String) + Send + Sync>;

/// What the worker reported about its environment.
#[derive(Debug, Clone, PartialEq)]
pub struct Identity {
    pub guile: String,
    pub effective_version: String,
    pub trust: Trust,
    pub load_path: Vec<String>,
    /// Channel name and commit pairs when Guix is present.
    pub guix_channels: Option<Vec<(String, String)>>,
    /// Stable string for cache keys: versions plus channels or load path.
    pub key: String,
    /// Incremented on every (re)start of the worker process.
    pub generation: u64,
}

impl Identity {
    fn from_hello(hello: &Datum, trust: Trust) -> Option<Identity> {
        let guile = hello.assq("guile")?.as_str()?.to_string();
        let effective_version = hello.assq("effective-version")?.as_str()?.to_string();
        let load_path: Vec<String> = hello
            .assq("load-path")
            .and_then(|entry| entry.as_list())
            .map(|items| {
                items
                    .iter()
                    .skip(1)
                    .filter_map(|d| d.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let guix_channels: Option<Vec<(String, String)>> = hello.assq("guix").and_then(|guix| {
            let channels = guix.assq("channels")?.as_list()?;
            Some(
                channels
                    .iter()
                    .skip(1)
                    .filter_map(|c| {
                        let pair = c.as_list()?;
                        Some((
                            pair[0].as_symbol()?.to_string(),
                            pair[1].as_str()?.to_string(),
                        ))
                    })
                    .collect(),
            )
        });
        let key = match &guix_channels {
            Some(channels) => format!(
                "guile-{guile}+guix:{}",
                channels
                    .iter()
                    .map(|(name, commit)| format!("{name}@{commit}"))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            None => format!("guile-{guile}:{}", load_path.join(":")),
        };
        Some(Identity {
            guile,
            effective_version,
            trust,
            load_path,
            guix_channels,
            key,
            generation: 0,
        })
    }

    pub fn summary(&self) -> String {
        match &self.guix_channels {
            Some(channels) => format!(
                "Guile {} with Guix ({} channels: {}), {} mode",
                self.guile,
                channels.len(),
                channels
                    .iter()
                    .map(|(name, commit)| format!("{name} {}", &commit[..commit.len().min(7)]))
                    .collect::<Vec<_>>()
                    .join(", "),
                self.trust.as_str()
            ),
            None => format!(
                "Guile {} without Guix, {} mode",
                self.guile,
                self.trust.as_str()
            ),
        }
    }
}

type Pending = Arc<StdMutex<HashMap<i64, oneshot::Sender<Incoming>>>>;

struct Process {
    child: Child,
    stdin: ChildStdin,
    pending: Pending,
    pid: u32,
}

struct State {
    process: Option<Process>,
    restarts: u32,
    generation: u64,
    disabled: Option<String>,
    identity: Option<Identity>,
    /// Load-path entries learned after start (workspace roots); applied to
    /// the running worker and to every restart.
    extra_load_path: Vec<String>,
}

struct Inner {
    config: WorkerConfig,
    logger: Logger,
    next_id: AtomicI64,
    /// Identity of the running worker, readable without the lane so requests
    /// can decide instantly whether the worker is up; `None` until the first
    /// handshake completes and after a crash until the restart.
    ready: StdMutex<Option<Identity>>,
    /// Requests are serialized: the worker is sequential and SIGUSR1 targets
    /// whatever it is running.
    lane: Mutex<State>,
    /// Pid of the running worker process, readable without the lane so a
    /// shutdown can kill a worker stuck in a long request.
    pid: AtomicU32,
    /// Set by `shutdown` before it takes the lane, so requests queued
    /// behind a stuck one fail at once instead of restarting the worker.
    shutting_down: AtomicBool,
}

#[derive(Clone)]
pub struct Worker {
    inner: Arc<Inner>,
}

impl Worker {
    pub fn new(config: WorkerConfig, logger: Logger) -> Worker {
        Worker {
            inner: Arc::new(Inner {
                config,
                logger,
                next_id: AtomicI64::new(1),
                ready: StdMutex::new(None),
                lane: Mutex::new(State {
                    process: None,
                    restarts: 0,
                    generation: 0,
                    disabled: None,
                    identity: None,
                    extra_load_path: Vec::new(),
                }),
                pid: AtomicU32::new(0),
                shutting_down: AtomicBool::new(false),
            }),
        }
    }

    pub fn config(&self) -> &WorkerConfig {
        &self.inner.config
    }

    /// The identity if the worker is up; `None` while it is starting or
    /// after it died. Editor requests use this so syntax features never wait
    /// for the worker to start; requests still queue behind each other.
    pub fn identity_if_ready(&self) -> Option<Identity> {
        self.inner.ready.lock().unwrap().clone()
    }

    /// The environment identity, starting the worker if needed.
    pub async fn identity(&self) -> Result<Identity, WorkerError> {
        let mut state = self.inner.lane.lock().await;
        self.ensure_process(&mut state).await?;
        Ok(state
            .identity
            .clone()
            .expect("identity set by ensure_process"))
    }

    /// Send a request and wait for its result. Dropping the returned future
    /// cancels the request in the worker.
    pub async fn request(&self, method: &str, params: Datum) -> Result<Datum, WorkerError> {
        let (result_tx, result_rx) = oneshot::channel();
        let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
        let worker = self.clone();
        let method = method.to_string();
        tokio::spawn(async move {
            let _ = result_tx.send(worker.run(&method, params, cancel_rx).await);
        });
        struct CancelOnDrop(Option<oneshot::Sender<()>>);
        impl Drop for CancelOnDrop {
            fn drop(&mut self) {
                if let Some(tx) = self.0.take() {
                    let _ = tx.send(());
                }
            }
        }
        let mut guard = CancelOnDrop(Some(cancel_tx));
        let result = result_rx.await.unwrap_or(Err(WorkerError::Cancelled));
        guard.0.take();
        result
    }

    pub async fn add_load_path(&self, path: String) {
        let mut state = self.inner.lane.lock().await;
        if state.extra_load_path.contains(&path) {
            return;
        }
        state.extra_load_path.push(path.clone());
        if state.process.is_some() {
            let params = Datum::list([Datum::Pair(
                Box::new(Datum::symbol("path")),
                Box::new(Datum::string(&path)),
            )]);
            let _ = self
                .request_locked(&mut state, "add-load-path", params)
                .await;
        }
    }

    /// A request issued while already holding the lane (handshake, load path).
    async fn request_locked(
        &self,
        state: &mut State,
        method: &str,
        params: Datum,
    ) -> Result<Datum, WorkerError> {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        let process = state
            .process
            .as_mut()
            .ok_or(WorkerError::Unavailable("not running".into()))?;
        process.pending.lock().unwrap().insert(id, tx);
        protocol::write_frame(&mut process.stdin, &protocol::request(id, method, params))
            .await
            .map_err(|e| WorkerError::Crashed(e.to_string()))?;
        match tokio::time::timeout(self.inner.config.deadline, rx).await {
            Ok(Ok(Incoming::Ok { result, .. })) => Ok(result),
            Ok(Ok(Incoming::Error { kind, message, .. })) => {
                Err(WorkerError::Remote { kind, message })
            }
            Ok(Ok(Incoming::Log { .. })) => unreachable!("logs are not routed to requests"),
            Ok(Err(_)) => Err(WorkerError::Crashed("worker exited".into())),
            Err(_) => Err(WorkerError::Timeout),
        }
    }

    pub async fn shutdown(&self) {
        self.inner.shutting_down.store(true, Ordering::Relaxed);
        // A worker deep in a long request holds the lane; do not wait for it.
        let mut state =
            match tokio::time::timeout(Duration::from_millis(500), self.inner.lane.lock()).await {
                Ok(state) => state,
                Err(_) => {
                    let pid = self.inner.pid.load(Ordering::Relaxed);
                    if pid != 0 {
                        // SAFETY: plain kill(2) on a pid this process spawned.
                        unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
                    }
                    self.inner.lane.lock().await
                }
            };
        if let Some(mut process) = state.process.take() {
            let _ = protocol::write_frame(&mut process.stdin, &protocol::shutdown()).await;
            let _ = tokio::time::timeout(Duration::from_secs(2), process.child.wait()).await;
            let _ = process.child.start_kill();
        }
        state.disabled = Some("server shut down".into());
        self.inner.pid.store(0, Ordering::Relaxed);
        *self.inner.ready.lock().unwrap() = None;
    }

    async fn run(
        &self,
        method: &str,
        params: Datum,
        mut cancel: oneshot::Receiver<()>,
    ) -> Result<Datum, WorkerError> {
        let mut state = self.inner.lane.lock().await;
        // A request cancelled before it reached the lane costs nothing.
        if cancel.try_recv().is_ok() {
            return Err(WorkerError::Cancelled);
        }
        if self.inner.shutting_down.load(Ordering::Relaxed) {
            return Err(WorkerError::Unavailable("server shut down".into()));
        }
        self.ensure_process(&mut state).await?;
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        let process = state.process.as_mut().expect("process running");
        process.pending.lock().unwrap().insert(id, tx);
        if let Err(err) =
            protocol::write_frame(&mut process.stdin, &protocol::request(id, method, params)).await
        {
            return Err(self.report_crash(&mut state, format!("cannot write to worker: {err}")));
        }
        let pid = process.pid;
        let deadline = tokio::time::sleep(self.inner.config.deadline);
        tokio::pin!(deadline);
        tokio::pin!(rx);
        let outcome = tokio::select! {
            reply = &mut rx => match reply {
                Ok(Incoming::Ok { result, .. }) => return Ok(result),
                Ok(Incoming::Error { kind, message, .. }) => return Err(WorkerError::Remote { kind, message }),
                Ok(Incoming::Log { .. }) => unreachable!("logs are not routed to requests"),
                Err(_) => Err(self.report_crash(&mut state, "worker exited during a request".into())),
            },
            _ = &mut deadline => Err(WorkerError::Timeout),
            _ = &mut cancel => Err(WorkerError::Cancelled),
        };
        // Deadline or cancellation: interrupt the worker and give it a moment to
        // acknowledge before treating it as hung.
        if matches!(
            outcome,
            Err(WorkerError::Timeout) | Err(WorkerError::Cancelled)
        ) {
            interrupt(pid);
            let acknowledged = tokio::time::timeout(Duration::from_millis(1500), &mut rx).await;
            match acknowledged {
                Ok(Ok(Incoming::Error { kind, .. })) if kind == "cancelled" => {}
                Ok(Ok(_)) => {} // finished after all; the caller no longer wants it
                _ => {
                    self.report_crash(
                        &mut state,
                        "worker did not respond to interruption; killed".into(),
                    );
                }
            }
        }
        outcome
    }

    async fn ensure_process(&self, state: &mut State) -> Result<(), WorkerError> {
        if let Some(reason) = &state.disabled {
            return Err(WorkerError::Unavailable(reason.clone()));
        }
        if state.process.is_some() {
            return Ok(());
        }
        let command = if self.inner.config.command.is_empty() {
            default_command().ok_or_else(|| {
                let reason = "no `guile` or `guix` found on PATH; set guile.command in the initialization options".to_string();
                state.disabled = Some(reason.clone());
                (self.inner.logger)(LogLevel::Error, reason.clone());
                WorkerError::Unavailable(reason)
            })?
        } else {
            self.inner.config.command.clone()
        };
        let process = match spawn(&command, &self.inner.config, &state.extra_load_path).await {
            Ok(process) => process,
            Err(err) => {
                let reason = format!("cannot start Guile worker with {command:?}: {err}");
                state.disabled = Some(reason.clone());
                (self.inner.logger)(LogLevel::Error, reason.clone());
                return Err(WorkerError::Unavailable(reason));
            }
        };
        self.inner.pid.store(process.pid, Ordering::Relaxed);
        state.process = Some(process);
        state.generation += 1;
        // Handshake: the identity doubles as a liveness check.
        let hello = self
            .request_locked(state, "hello", Datum::list([]))
            .await
            .ok();
        match hello.and_then(|hello| Identity::from_hello(&hello, self.inner.config.trust)) {
            Some(mut identity) => {
                identity.generation = state.generation;
                (self.inner.logger)(
                    LogLevel::Info,
                    format!("Guile worker ready: {}", identity.summary()),
                );
                *self.inner.ready.lock().unwrap() = Some(identity.clone());
                state.identity = Some(identity);
                Ok(())
            }
            None => {
                let reason = format!(
                    "Guile worker started with {command:?} but did not answer the hello handshake"
                );
                if let Some(mut process) = state.process.take() {
                    let _ = process.child.start_kill();
                }
                state.disabled = Some(reason.clone());
                (self.inner.logger)(LogLevel::Error, reason.clone());
                Err(WorkerError::Unavailable(reason))
            }
        }
    }

    /// Drop the current process and decide whether a restart is allowed.
    fn report_crash(&self, state: &mut State, why: String) -> WorkerError {
        if let Some(mut process) = state.process.take() {
            let _ = process.child.start_kill();
        }
        *self.inner.ready.lock().unwrap() = None;
        state.restarts += 1;
        let max = self.inner.config.max_restarts;
        if state.restarts > max {
            let reason = format!("{why}; giving up after {max} restarts");
            state.disabled = Some(reason.clone());
            (self.inner.logger)(LogLevel::Error, reason);
        } else {
            (self.inner.logger)(
                LogLevel::Warning,
                format!(
                    "{why}; will restart on the next request ({}/{max})",
                    state.restarts
                ),
            );
        }
        WorkerError::Crashed(why)
    }
}

fn interrupt(pid: u32) {
    // SAFETY: plain signal delivery to a child we spawned.
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGUSR1);
    }
}

async fn spawn(
    command: &[String],
    config: &WorkerConfig,
    extra_load_path: &[String],
) -> std::io::Result<Process> {
    let dir = install_sources()?;
    let mut args: Vec<String> = command[1..].to_vec();
    let mut load_args = vec!["-L".to_string(), dir.to_string_lossy().into_owned()];
    for path in config.load_path.iter().chain(extra_load_path) {
        load_args.push("-L".into());
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
    args.push(dir.join("worker.scm").to_string_lossy().into_owned());
    args.push("--trust".into());
    args.push(config.trust.as_str().into());
    let mut child = Command::new(&command[0])
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()?;
    let stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let pid = child.id().unwrap_or_default();
    let pending: Pending = Arc::default();
    let routes = pending.clone();
    tokio::spawn(async move {
        let mut reader = BufReader::new(stdout);
        while let Ok(Some(datum)) = protocol::read_frame(&mut reader).await {
            match Incoming::parse(&datum) {
                Some(Incoming::Log { level, message }) => {
                    eprintln!("guile-lsp worker [{level}]: {message}");
                }
                Some(message @ (Incoming::Ok { .. } | Incoming::Error { .. })) => {
                    let id = match &message {
                        Incoming::Ok { id, .. } | Incoming::Error { id, .. } => *id,
                        Incoming::Log { .. } => unreachable!(),
                    };
                    if let Some(tx) = routes.lock().unwrap().remove(&id) {
                        let _ = tx.send(message);
                    }
                }
                None => eprintln!("guile-lsp worker: ignoring message {datum}"),
            }
        }
        // Dropping the senders tells waiting requests the worker is gone.
        routes.lock().unwrap().clear();
    });
    Ok(Process {
        child,
        stdin,
        pending,
        pid,
    })
}

const ENTRY_SCM: &str = include_str!("../../guile/worker.scm");
const WORKER_SCM: &str = include_str!("../../guile/guile-lsp/worker.scm");
const METADATA_SCM: &str = include_str!("../../guile/guile-lsp/metadata.scm");
const VALIDATE_ENTRY_SCM: &str = include_str!("../../guile/validate.scm");
const VALIDATE_SCM: &str = include_str!("../../guile/guile-lsp/validate.scm");

/// Materialize the embedded Guile sources so the binary is self-contained.
/// `GUILE_LSP_WORKER_DIR` overrides this with a checkout's `guile/` directory.
pub fn install_sources() -> std::io::Result<PathBuf> {
    if let Ok(dir) = std::env::var("GUILE_LSP_WORKER_DIR") {
        return Ok(PathBuf::from(dir));
    }
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in [
        ENTRY_SCM,
        WORKER_SCM,
        METADATA_SCM,
        VALIDATE_ENTRY_SCM,
        VALIDATE_SCM,
    ]
    .concat()
    .bytes()
    {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    let dir = std::env::temp_dir().join(format!("guile-lsp-worker-{hash:016x}"));
    let module_dir = dir.join("guile-lsp");
    std::fs::create_dir_all(&module_dir)?;
    write_if_missing(&dir.join("worker.scm"), ENTRY_SCM)?;
    write_if_missing(&module_dir.join("worker.scm"), WORKER_SCM)?;
    write_if_missing(&module_dir.join("metadata.scm"), METADATA_SCM)?;
    write_if_missing(&dir.join("validate.scm"), VALIDATE_ENTRY_SCM)?;
    write_if_missing(&module_dir.join("validate.scm"), VALIDATE_SCM)?;
    Ok(dir)
}

fn write_if_missing(path: &Path, contents: &str) -> std::io::Result<()> {
    if std::fs::read_to_string(path).ok().as_deref() != Some(contents) {
        std::fs::write(path, contents)?;
    }
    Ok(())
}

/// `guile` on PATH, else `guix repl --`, else nothing.
pub fn default_command() -> Option<Vec<String>> {
    let path = std::env::var_os("PATH")?;
    let on_path = |name: &str| std::env::split_paths(&path).any(|dir| dir.join(name).is_file());
    if on_path("guile") {
        Some(vec!["guile".into()])
    } else if on_path("guix") {
        Some(vec!["guix".into(), "repl".into(), "--".into()])
    } else {
        None
    }
}
