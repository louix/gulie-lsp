use guile_lsp::adapters;
use guile_lsp::adapters::validate::{self as validation, Validation};
use guile_lsp::cache::LookupCache;
use guile_lsp::completion::{self, Context};
use guile_lsp::config::Settings;
use guile_lsp::diagnostics;
use guile_lsp::documents::{Documents, Snapshot};
use guile_lsp::features;
use guile_lsp::format::{self, Rules};
use guile_lsp::imports;
use guile_lsp::resolver::{exports_params, Resolved, Resolver};
use guile_lsp::roots;
use guile_lsp::scope::{self, BindingKind};
use guile_lsp::semantic;
use guile_lsp::syntax::datum::Datum;
use guile_lsp::worker::protocol::WorkerError;
use guile_lsp::worker::{LogLevel, Worker};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::request::{GotoTypeDefinitionParams, GotoTypeDefinitionResponse};
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

const ANALYSIS_DEBOUNCE: Duration = Duration::from_millis(300);

type SemanticDiagnostics = HashMap<Url, (i32, Vec<Diagnostic>)>;

struct Backend {
    client: Client,
    documents: RwLock<Documents>,
    settings: RwLock<Settings>,
    worker: OnceLock<Worker>,
    /// Workspace folders plus roots inferred from open files.
    roots: Mutex<BTreeSet<PathBuf>>,
    lookups: Arc<Mutex<LookupCache>>,
    /// Imports whose export listings were requested ahead of time, per
    /// document, so a changed import list triggers warming again.
    warmed: Mutex<HashMap<Url, Vec<scope::Import>>>,
    analyses: Mutex<HashMap<Url, JoinHandle<()>>>,
    semantic: Arc<Mutex<SemanticDiagnostics>>,
    restricted_notified: Arc<AtomicBool>,
    /// Indentation rules declared by `.dir-locals.el` files in the roots.
    dir_locals: Mutex<Vec<(String, u8)>>,
    validations: Mutex<HashMap<Url, JoinHandle<()>>>,
    validated: Arc<Mutex<SemanticDiagnostics>>,
    /// For commands sent without a URI argument.
    recent: Mutex<Option<Url>>,
    warming: Arc<Mutex<HashSet<String>>>,
}

impl Backend {
    fn resolver(&self) -> Option<Resolver> {
        Some(Resolver {
            worker: self.worker.get()?.clone(),
            cache: self.lookups.clone(),
        })
    }

    async fn publish(&self, uri: Url, snapshot: &Snapshot) {
        let mut all = features::diagnostics(snapshot);
        if let Some((version, semantic)) = self.semantic.lock().unwrap().get(&uri) {
            if *version == snapshot.version {
                all.extend(semantic.iter().cloned());
            }
        }
        if let Some((version, validation)) = self.validated.lock().unwrap().get(&uri) {
            if *version == snapshot.version {
                all.extend(validation.iter().cloned());
            }
        }
        self.client
            .publish_diagnostics(uri, all, Some(snapshot.version))
            .await;
    }

    async fn snapshot(&self, uri: &Url) -> Option<Snapshot> {
        self.documents.read().await.get(uri).cloned()
    }

    fn logger(&self) -> guile_lsp::worker::Logger {
        let client = self.client.clone();
        Arc::new(move |level, message| {
            let client = client.clone();
            let kind = match level {
                LogLevel::Info => MessageType::INFO,
                LogLevel::Warning => MessageType::WARNING,
                LogLevel::Error => MessageType::ERROR,
            };
            tokio::spawn(async move {
                client.log_message(kind, message).await;
            });
        })
    }

    async fn note_root(&self, root: PathBuf) {
        let new = self.roots.lock().unwrap().insert(root.clone());
        if new {
            if let Ok(text) = std::fs::read_to_string(root.join(".dir-locals.el")) {
                let rules = format::dir_locals_rules(&text);
                if !rules.is_empty() {
                    self.client
                        .log_message(
                            MessageType::INFO,
                            format!(
                                "{} indentation rules from {}",
                                rules.len(),
                                root.join(".dir-locals.el").display()
                            ),
                        )
                        .await;
                    self.dir_locals.lock().unwrap().extend(rules);
                }
            }
            // Telling the worker waits for its lane, which may be busy starting
            // up; nothing in a notification handler should wait for that.
            if let Some(worker) = self.worker.get().cloned() {
                let path = root.to_string_lossy().into_owned();
                tokio::spawn(async move { worker.add_load_path(path).await });
            }
        }
    }

    async fn rules(&self, snapshot: &Snapshot) -> Rules {
        let mut rules = Rules::scheme();
        let under_guix = self
            .worker
            .get()
            .and_then(|w| w.identity_if_ready())
            .is_some_and(|identity| adapters::guix::active(&identity));
        if under_guix || adapters::guix::imports_guix(snapshot) {
            rules.extend(adapters::guix::indent_rules());
            rules.treat_as_prefixes(adapters::guix::INDENT_PREFIXES);
        }
        let locals = self.dir_locals.lock().unwrap().clone();
        rules.extend(locals.iter().map(|(name, n)| (name.as_str(), *n)));
        format::add_document_rules(&mut rules, snapshot);
        let settings = self.settings.read().await;
        rules.extend(settings.indent.iter().map(|(name, n)| (name.as_str(), *n)));
        rules
    }

    async fn unsaved_documents(&self) -> Vec<String> {
        let documents = self.documents.read().await;
        documents
            .iter()
            .filter_map(|(uri, snapshot)| {
                let path = uri.to_file_path().ok()?;
                let on_disk = std::fs::read_to_string(&path).ok()?;
                (on_disk != snapshot.text.as_ref()).then(|| {
                    path.file_name()
                        .unwrap_or(path.as_os_str())
                        .to_string_lossy()
                        .into_owned()
                })
            })
            .collect()
    }

    async fn start_validation(&self, uri: Url) -> Result<()> {
        let refuse = |text: String| Err(tower_lsp::jsonrpc::Error::invalid_params(text));
        let Ok(path) = uri.to_file_path() else {
            return refuse(format!("{uri} is not a local file"));
        };
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let Some(snapshot) = self.snapshot(&uri).await else {
            return refuse(format!("{name} is not open"));
        };
        let unsaved = self.unsaved_documents().await;
        if !unsaved.is_empty() {
            return refuse(format!(
                "Save {} before validating: validation loads files from disk, exactly as `guix system` would, so unsaved buffers would not be seen",
                unsaved.join(", ")
            ));
        }
        let settings = self.settings.read().await;
        let command = if settings.worker.command.is_empty() {
            guile_lsp::worker::default_command()
        } else {
            Some(settings.worker.command.clone())
        };
        let Some(command) = command else {
            return refuse(
                "No Guile command: set guile.command to [\"guix\", \"repl\", \"--\"]".into(),
            );
        };
        if let Some(identity) = self.worker.get().and_then(|w| w.identity_if_ready()) {
            if !adapters::guix::active(&identity) {
                return refuse(format!(
                    "The Guile worker ({}) is not Guix; validation needs guile.command set to [\"guix\", \"repl\", \"--\"]",
                    identity.guile
                ));
            }
        }
        let script_dir = match guile_lsp::worker::install_sources() {
            Ok(dir) => dir,
            Err(e) => return refuse(format!("cannot install the validation script: {e}")),
        };
        let mut load_path = settings.worker.load_path.clone();
        for root in self.roots.lock().unwrap().iter() {
            let root = root.to_string_lossy().into_owned();
            if !load_path.contains(&root) {
                load_path.push(root);
            }
        }
        let config = validation::Config {
            command,
            load_path,
            timeout: settings.validate_timeout,
            script_dir,
        };
        drop(settings);

        self.cancel_validation(&uri, false).await;
        self.client
            .log_message(MessageType::INFO, format!("Validating {name}..."))
            .await;
        let client = self.client.clone();
        let validated = self.validated.clone();
        let version = snapshot.version;
        let task_uri = uri.clone();
        let task = tokio::spawn(async move {
            let outcome = validation::validate(&config, &path).await;
            let message = match outcome {
                Ok(report) => {
                    let (diagnostics, message) = validation_results(&snapshot, &path, &report);
                    validated
                        .lock()
                        .unwrap()
                        .insert(task_uri.clone(), (version, diagnostics.clone()));
                    let mut all = features::diagnostics(&snapshot);
                    all.extend(diagnostics);
                    client
                        .publish_diagnostics(task_uri, all, Some(version))
                        .await;
                    message
                }
                Err(failure) => (MessageType::ERROR, format!("{name}: {failure}")),
            };
            client.show_message(message.0, message.1).await;
        });
        if let Some(previous) = self.validations.lock().unwrap().insert(uri, task) {
            previous.abort();
        }
        Ok(())
    }

    async fn cancel_validation(&self, uri: &Url, announce: bool) -> bool {
        let task = self.validations.lock().unwrap().remove(uri);
        match task {
            Some(task) => {
                task.abort();
                if announce {
                    let name = uri
                        .path_segments()
                        .and_then(|mut s| s.next_back())
                        .unwrap_or("document");
                    self.client
                        .show_message(MessageType::INFO, format!("Validation of {name} cancelled"))
                        .await;
                }
                true
            }
            None => false,
        }
    }

    async fn module_location(&self, snapshot: &Snapshot, position: Position) -> Option<Location> {
        let module = semantic::module_at(snapshot, position)?;
        let found = self
            .cached("module-file", semantic::module_params(&module))
            .await?;
        let file = found.assq("file")?.as_str()?;
        let uri = Url::from_file_path(file).ok()?;
        Some(Location::new(uri, Range::default()))
    }

    async fn format(
        &self,
        uri: &Url,
        lines: Option<std::ops::Range<u32>>,
    ) -> Option<Vec<TextEdit>> {
        let snapshot = self.snapshot(uri).await?;
        let rules = self.rules(&snapshot).await;
        let outcome = format::format(&snapshot, &rules, lines.clone());
        if let Some(stop) = outcome.stopped {
            let requested = lines.is_none_or(|range| range.end > stop.line);
            if requested {
                self.client
                    .show_message(
                        MessageType::WARNING,
                        format!(
                            "Formatted only up to line {}: {} there",
                            stop.line + 1,
                            stop.message
                        ),
                    )
                    .await;
            }
        }
        Some(outcome.edits)
    }

    async fn note_document_root(&self, uri: &Url, snapshot: &Snapshot) {
        if let Some(root) = roots::root_for(uri, snapshot) {
            self.note_root(root).await;
        }
        // A file importing a local module names where that module lives.
        if let Ok(file) = uri.to_file_path() {
            let modules: Vec<Vec<String>> = scope::imports(snapshot)
                .into_iter()
                .map(|i| i.module)
                .collect();
            for root in roots::import_roots(&file, &modules) {
                self.note_root(root).await;
            }
        }
    }

    fn imports(&self, snapshot: &Snapshot) -> Vec<scope::Import> {
        let mut imports = scope::imports(snapshot);
        if let Some(identity) = self.worker.get().and_then(|w| w.identity_if_ready()) {
            if adapters::guix::active(&identity) {
                for extra in adapters::guix::shorthand_imports(snapshot) {
                    if !imports.contains(&extra) {
                        imports.push(extra);
                    }
                }
            }
        }
        imports
    }

    /// Report a failed resolution once (restricted) or every time (others).
    async fn report(&self, what: &str, outcome: &Resolved) {
        match outcome {
            Resolved::Restricted(message) => {
                if !self.restricted_notified.swap(true, Ordering::Relaxed) {
                    self.client
                        .log_message(
                            MessageType::INFO,
                            format!("{message}; set guile.trust to \"trusted\" in the initialization options"),
                        )
                        .await;
                }
            }
            Resolved::Failed(err) => {
                self.client
                    .log_message(MessageType::WARNING, format!("{what} failed: {err}"))
                    .await;
            }
            _ => {}
        }
    }

    async fn cached(&self, method: &str, params: Datum) -> Option<Datum> {
        let resolver = self.resolver()?;
        let outcome = resolver.request(method, params.clone()).await;
        match outcome {
            Resolved::Found(result) => Some(result),
            other => {
                self.report(&format!("{method} {params}"), &other).await;
                None
            }
        }
    }

    async fn lookup(&self, snapshot: &Snapshot, name: &str) -> Option<Datum> {
        let resolver = self.resolver()?;
        match resolver.lookup(&self.imports(snapshot), name).await {
            Ok(found) => found,
            Err(outcome) => {
                self.report(&format!("lookup of {name}"), &outcome).await;
                None
            }
        }
    }

    fn lexical(
        &self,
        snapshot: &Snapshot,
        position: Position,
        name: &str,
    ) -> Option<scope::Binding> {
        let offset = snapshot.offset(position)?;
        scope::resolve_lexical(snapshot, offset, name)
    }

    async fn open_definition(
        &self,
        uri: &Url,
        snapshot: &Snapshot,
        name: &str,
    ) -> Option<(Url, DocumentSymbol, String)> {
        let (uri, definition, text) =
            if let Some(definition) = semantic::local_definition(snapshot, name) {
                (uri.clone(), definition, snapshot.text.clone())
            } else {
                let documents = self.documents.read().await;
                let (other, definition) =
                    semantic::definition_in_open_documents(documents.iter(), snapshot, name)?;
                let text = documents.get(&other)?.text.clone();
                (other, definition, text)
            };
        let line = text
            .lines()
            .nth(definition.range.start.line as usize)
            .unwrap_or("")
            .trim()
            .to_string();
        Some((uri, definition, line))
    }

    /// Never awaits the worker: this runs from notification handlers, and
    /// starting the worker must not delay diagnostics.
    fn warm_imports(&self, uri: &Url, snapshot: &Snapshot) {
        let imports = scope::imports(snapshot);
        {
            let mut warmed = self.warmed.lock().unwrap();
            if warmed.get(uri) == Some(&imports) {
                return;
            }
            warmed.insert(uri.clone(), imports.clone());
        }
        let Some(resolver) = self.resolver() else {
            return;
        };
        let snapshot = snapshot.clone();
        tokio::spawn(async move {
            let Ok(identity) = resolver.worker.identity().await else {
                return;
            };
            let mut imports = imports;
            if adapters::guix::active(&identity) {
                for extra in adapters::guix::shorthand_imports(&snapshot) {
                    if !imports.contains(&extra) {
                        imports.push(extra);
                    }
                }
            }
            for import in imports {
                resolver.request("exports", exports_params(&import)).await;
            }
        });
    }

    fn schedule_analysis(&self, uri: Url, snapshot: Snapshot) {
        let Some(resolver) = self.resolver() else {
            return;
        };
        let client = self.client.clone();
        let semantic = self.semantic.clone();
        let restricted_notified = self.restricted_notified.clone();
        let imports = self.imports(&snapshot);
        let task = tokio::spawn({
            let uri = uri.clone();
            async move {
                tokio::time::sleep(ANALYSIS_DEBOUNCE).await;
                let Some(found) = diagnostics::analyze(&resolver, &snapshot, &imports).await else {
                    // Worker not ready or workspace restricted: syntax only.
                    if resolver.identity().is_some()
                        && !restricted_notified.swap(true, Ordering::Relaxed)
                    {
                        client
                            .log_message(
                                MessageType::INFO,
                                "semantic diagnostics need workspace trust; set guile.trust to \"trusted\" in the initialization options",
                            )
                            .await;
                    }
                    return;
                };
                semantic
                    .lock()
                    .unwrap()
                    .insert(uri.clone(), (snapshot.version, found.clone()));
                let mut all = features::diagnostics(&snapshot);
                all.extend(found);
                client
                    .publish_diagnostics(uri, all, Some(snapshot.version))
                    .await;
            }
        });
        if let Some(previous) = self.analyses.lock().unwrap().insert(uri, task) {
            previous.abort();
        }
    }

    /// Without `wait`, only cached answers count and a miss is fetched in the
    /// background: completion must never queue behind the worker.
    async fn record_fields(
        &self,
        snapshot: &Snapshot,
        context: &adapters::guix::RecordContext,
        wait: bool,
    ) -> Option<Datum> {
        let identity = self.worker.get()?.identity_if_ready()?;
        if !adapters::guix::active(&identity) {
            return None;
        }
        let lookup = if wait {
            self.lookup(snapshot, &context.head).await?
        } else {
            let resolver = self.resolver()?;
            let imports = self.imports(snapshot);
            let params = semantic::lookup_params_for(&imports, &context.head);
            match params.iter().find_map(|p| resolver.cached("lookup", p)) {
                Some(found) => found,
                None => {
                    for p in params {
                        self.warm(&resolver, "lookup", p);
                    }
                    return None;
                }
            }
        };
        let module = adapters::guix::lookup_module(&lookup)?;
        let exported = lookup
            .assq("binding")?
            .assq("name")?
            .as_symbol()?
            .to_string();
        let params = adapters::guix::configuration_params(&module, &exported);
        let fields = if wait {
            self.cached("configuration", params).await?
        } else {
            let resolver = self.resolver()?;
            match resolver.cached("configuration", &params) {
                Some(found) => found,
                None => {
                    self.warm(&resolver, "configuration", params);
                    return None;
                }
            }
        };
        adapters::guix::is_record(&fields).then_some(fields)
    }

    async fn field_at(&self, snapshot: &Snapshot, position: Position) -> Option<(Datum, String)> {
        let offset = snapshot.offset(position)?;
        let context = adapters::guix::record_context(snapshot, offset)?;
        let field = context.field.clone()?;
        let fields = self.record_fields(snapshot, &context, true).await?;
        Some((fields, field))
    }

    fn warm(&self, resolver: &Resolver, method: &'static str, params: Datum) {
        let key = format!("{method} {params}");
        if !self.warming.lock().unwrap().insert(key.clone()) {
            return;
        }
        let resolver = resolver.clone();
        let warming = self.warming.clone();
        tokio::spawn(async move {
            let _ = resolver.request(method, params).await;
            warming.lock().unwrap().remove(&key);
        });
    }

    async fn reference_items(
        &self,
        snapshot: &Snapshot,
        offset: usize,
        prefix: &str,
        position: Position,
    ) -> Vec<CompletionItem> {
        let mut seen = BTreeSet::new();
        let mut items = completion::lexical_items(snapshot, offset, prefix, &mut seen);
        items.extend(completion::special_form_items(prefix, &mut seen));
        // A miss is fetched once in the background for the next keystroke.
        if let Some(resolver) = self.resolver() {
            for import in self.imports(snapshot) {
                let params = exports_params(&import);
                match resolver.cached("exports", &params) {
                    Some(exports) => items.extend(completion::imported_items(
                        &import, &exports, prefix, &mut seen,
                    )),
                    None => self.warm(&resolver, "exports", params),
                }
            }
        }
        let mut labels: HashSet<String> = items.iter().map(|i| i.label.clone()).collect();
        items.extend(
            features::completions(snapshot, position)
                .into_iter()
                .filter(|i| labels.insert(i.label.clone())),
        );
        items
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        let mut settings =
            Settings::from_initialization_options(params.initialization_options.as_ref());
        let mut folders: Vec<PathBuf> = params
            .workspace_folders
            .unwrap_or_default()
            .iter()
            .filter_map(|f| f.uri.to_file_path().ok())
            .collect();
        #[allow(deprecated)]
        if let Some(root) = params.root_uri.as_ref().and_then(|u| u.to_file_path().ok()) {
            folders.push(root);
        }
        for folder in folders {
            if self.roots.lock().unwrap().insert(folder.clone()) {
                settings
                    .worker
                    .load_path
                    .push(folder.to_string_lossy().into_owned());
            }
        }
        let _ = self
            .worker
            .set(Worker::new(settings.worker.clone(), self.logger()));
        *self.settings.write().await = settings;
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                position_encoding: Some(PositionEncodingKind::UTF16),
                text_document_sync: Some(TextDocumentSyncCapability::Options(
                    TextDocumentSyncOptions {
                        open_close: Some(true),
                        change: Some(TextDocumentSyncKind::FULL),
                        save: Some(TextDocumentSyncSaveOptions::Supported(true)),
                        ..Default::default()
                    },
                )),
                document_symbol_provider: Some(OneOf::Left(true)),
                selection_range_provider: Some(SelectionRangeProviderCapability::Simple(true)),
                completion_provider: Some(CompletionOptions::default()),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                definition_provider: Some(OneOf::Left(true)),
                code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
                type_definition_provider: Some(TypeDefinitionProviderCapability::Simple(true)),
                document_formatting_provider: Some(OneOf::Left(true)),
                document_range_formatting_provider: Some(OneOf::Left(true)),
                execute_command_provider: Some(ExecuteCommandOptions {
                    commands: vec!["guile.validate".into(), "guile.validate.cancel".into()],
                    work_done_progress_options: Default::default(),
                }),
                ..Default::default()
            },
            server_info: Some(ServerInfo {
                name: "guile-lsp".into(),
                version: Some(env!("CARGO_PKG_VERSION").into()),
            }),
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(
                MessageType::INFO,
                "Guile LSP ready: syntax diagnostics, symbols, selection, completion; hover, definition, and semantic diagnostics through the Guile worker.",
            )
            .await;
        for problem in &self.settings.read().await.problems {
            self.client
                .log_message(MessageType::WARNING, format!("ignoring setting: {problem}"))
                .await;
        }
        // Start eagerly so environment problems surface now, not on the first hover.
        if let Some(worker) = self.worker.get().cloned() {
            let client = self.client.clone();
            tokio::spawn(async move {
                if let Err(WorkerError::Unavailable(reason)) = worker.identity().await {
                    client
                        .show_message(
                            MessageType::WARNING,
                            format!("Guile worker unavailable: {reason}"),
                        )
                        .await;
                }
            });
        }
    }

    async fn shutdown(&self) -> Result<()> {
        for (_, task) in self.validations.lock().unwrap().drain() {
            task.abort();
        }
        if let Some(worker) = self.worker.get() {
            worker.shutdown().await;
        }
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        *self.recent.lock().unwrap() = Some(uri.clone());
        let snapshot = self.documents.write().await.open(params.text_document);
        self.note_document_root(&uri, &snapshot).await;
        self.warm_imports(&uri, &snapshot);
        self.publish(uri.clone(), &snapshot).await;
        self.schedule_analysis(uri, snapshot);
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        *self.recent.lock().unwrap() = Some(uri.clone());
        self.validated.lock().unwrap().remove(&uri);
        let result = self.documents.write().await.change(params);
        match result {
            Ok(snapshot) => {
                self.note_document_root(&uri, &snapshot).await;
                self.warm_imports(&uri, &snapshot);
                self.publish(uri.clone(), &snapshot).await;
                self.schedule_analysis(uri, snapshot);
            }
            Err(reason) => self.client.log_message(MessageType::WARNING, reason).await,
        }
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        // The worker reads modules from disk: anything cached may be stale now.
        self.lookups.lock().unwrap().clear();
        if let Some(snapshot) = self.snapshot(&params.text_document.uri).await {
            self.schedule_analysis(params.text_document.uri, snapshot);
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        self.documents.write().await.close(&uri);
        self.warmed.lock().unwrap().remove(&uri);
        self.semantic.lock().unwrap().remove(&uri);
        if let Some(task) = self.analyses.lock().unwrap().remove(&uri) {
            task.abort();
        }
        self.validated.lock().unwrap().remove(&uri);
        if let Some(task) = self.validations.lock().unwrap().remove(&uri) {
            task.abort();
        }
        self.client.publish_diagnostics(uri, Vec::new(), None).await;
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        Ok(self
            .snapshot(&params.text_document.uri)
            .await
            .map(|snapshot| DocumentSymbolResponse::Nested(features::document_symbols(&snapshot))))
    }

    async fn selection_range(
        &self,
        params: SelectionRangeParams,
    ) -> Result<Option<Vec<SelectionRange>>> {
        Ok(self
            .snapshot(&params.text_document.uri)
            .await
            .map(|snapshot| {
                params
                    .positions
                    .iter()
                    .filter_map(|position| features::selection_range(&snapshot, *position))
                    .collect()
            }))
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let position = params.text_document_position;
        let Some(snapshot) = self.snapshot(&position.text_document.uri).await else {
            return Ok(None);
        };
        let Some(offset) = snapshot.offset(position.position) else {
            return Ok(None);
        };
        if let Some(context) = adapters::guix::record_context(&snapshot, offset) {
            if let Some(fields) = self.record_fields(&snapshot, &context, false).await {
                return Ok(Some(CompletionResponse::Array(
                    adapters::guix::field_items(&fields, &context),
                )));
            }
        }
        let items = match completion::context(&snapshot, offset) {
            Context::Quoted => Vec::new(),
            Context::ModulePath { typed, prefix } => match self.resolver() {
                Some(resolver) => match resolver.cached("list-modules", &Datum::list([])) {
                    Some(modules) => completion::module_path_items(&modules, &typed, &prefix),
                    None => {
                        self.warm(&resolver, "list-modules", Datum::list([]));
                        Vec::new()
                    }
                },
                None => Vec::new(),
            },
            Context::Reference { prefix } => {
                self.reference_items(&snapshot, offset, &prefix, position.position)
                    .await
            }
        };
        Ok(Some(CompletionResponse::Array(items)))
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let position = params.text_document_position_params;
        let uri = position.text_document.uri;
        let Some(snapshot) = self.snapshot(&uri).await else {
            return Ok(None);
        };
        if let Some(module) = semantic::module_at(&snapshot, position.position) {
            let file = self
                .cached("module-file", semantic::module_params(&module))
                .await
                .and_then(|found| found.assq("file")?.as_str().map(str::to_string));
            let value = match file {
                Some(file) => format!("**module** `({})`\n\n`{file}`", module.join(" ")),
                None => format!(
                    "**module** `({})`\n\nnot found on the load path",
                    module.join(" ")
                ),
            };
            return Ok(Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value,
                }),
                range: None,
            }));
        }
        let Some((node, name)) = semantic::symbol_at(&snapshot, position.position) else {
            return Ok(None);
        };
        let range = Some(snapshot.range(node.range.clone()));
        let markdown = |value: String| Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value,
            }),
            range,
        };
        if let Some((fields, field)) = self.field_at(&snapshot, position.position).await {
            if let Some(text) = adapters::guix::field_hover(&fields, &field) {
                return Ok(Some(markdown(text)));
            }
        }
        if let Some(binding) = self
            .lexical(&snapshot, position.position, name)
            .filter(|b| b.kind == BindingKind::Local)
        {
            let line = snapshot.position(binding.range.start).line + 1;
            return Ok(Some(markdown(format!(
                "```scheme\n{name}\n```\n*local binding* introduced at line {line}"
            ))));
        }
        if let Some((defined_in, definition, line)) =
            self.open_definition(&uri, &snapshot, name).await
        {
            let place = if defined_in == uri {
                "this file".to_string()
            } else {
                defined_in
                    .to_file_path()
                    .ok()
                    .and_then(|p| p.file_name().map(|f| f.to_string_lossy().into_owned()))
                    .unwrap_or_else(|| defined_in.to_string())
            };
            return Ok(Some(markdown(format!(
                "```scheme\n{line}\n```\n*{}* defined in {place} at line {}",
                definition.detail.as_deref().unwrap_or("definition"),
                definition.range.start.line + 1
            ))));
        }
        let Some(lookup) = self.lookup(&snapshot, name).await else {
            return Ok(None);
        };
        let mut value = semantic::hover_markdown(name, &lookup);
        // Library adapters may add to the hover; their failures are silent.
        if let Some(worker) = self.worker.get() {
            if let Some(identity) = worker.identity_if_ready() {
                if let Some(extra) =
                    adapters::guix::enrich_hover(worker, &identity, name, &lookup).await
                {
                    value.push_str(&extra);
                }
            }
        }
        Ok(Some(markdown(value)))
    }

    async fn formatting(&self, params: DocumentFormattingParams) -> Result<Option<Vec<TextEdit>>> {
        Ok(self.format(&params.text_document.uri, None).await)
    }

    async fn range_formatting(
        &self,
        params: DocumentRangeFormattingParams,
    ) -> Result<Option<Vec<TextEdit>>> {
        let range = params.range;
        // A range ending at column 0 stops before that line.
        let end = if range.end.character == 0 && range.end.line > range.start.line {
            range.end.line
        } else {
            range.end.line + 1
        };
        Ok(self
            .format(&params.text_document.uri, Some(range.start.line..end))
            .await)
    }

    async fn execute_command(&self, params: ExecuteCommandParams) -> Result<Option<Value>> {
        let uri = params
            .arguments
            .first()
            .and_then(Value::as_str)
            .and_then(|s| Url::parse(s).ok())
            .or_else(|| self.recent.lock().unwrap().clone());
        match params.command.as_str() {
            "guile.validate" => {
                let Some(uri) = uri else {
                    return Err(tower_lsp::jsonrpc::Error::invalid_params(
                        "guile.validate takes a document URI, and no document is open",
                    ));
                };
                // Editors rarely surface an error reply to a workspace command,
                // so a refusal is also shown as a message.
                if let Err(error) = self.start_validation(uri).await {
                    self.client
                        .show_message(MessageType::WARNING, error.message.to_string())
                        .await;
                    return Err(error);
                }
                Ok(Some(json!({ "started": true })))
            }
            "guile.validate.cancel" => {
                let cancelled = match uri {
                    Some(uri) => self.cancel_validation(&uri, true).await,
                    None => {
                        let uris: Vec<Url> =
                            self.validations.lock().unwrap().keys().cloned().collect();
                        let mut any = false;
                        for uri in uris {
                            any |= self.cancel_validation(&uri, true).await;
                        }
                        any
                    }
                };
                Ok(Some(json!({ "cancelled": cancelled })))
            }
            other => Err(tower_lsp::jsonrpc::Error::invalid_params(format!(
                "unknown command {other}"
            ))),
        }
    }

    /// One quick fix per loaded module that exports a name the document
    /// neither binds nor imports.
    async fn code_action(&self, params: CodeActionParams) -> Result<Option<CodeActionResponse>> {
        let uri = params.text_document.uri;
        let Some(snapshot) = self.snapshot(&uri).await else {
            return Ok(None);
        };
        let Some((_, name)) = semantic::symbol_at(&snapshot, params.range.start) else {
            return Ok(None);
        };
        if self.lexical(&snapshot, params.range.start, name).is_some()
            || semantic::local_definition(&snapshot, name).is_some()
        {
            return Ok(None);
        }
        let Some(resolver) = self.resolver() else {
            return Ok(None);
        };
        let imports = self.imports(&snapshot);
        if matches!(resolver.lookup(&imports, name).await, Ok(Some(_))) {
            return Ok(None);
        }
        // Not cached: the set of loaded modules grows as the session goes on.
        let found = match resolver
            .worker
            .request(
                "find-exporters",
                Datum::list([Datum::Pair(
                    Box::new(Datum::symbol("name")),
                    Box::new(Datum::string(name)),
                )]),
            )
            .await
        {
            Ok(found) => found,
            Err(err) => {
                self.client
                    .log_message(
                        MessageType::WARNING,
                        format!("import assistance for {name} failed: {err}"),
                    )
                    .await;
                return Ok(None);
            }
        };
        // `assq` on a two-element entry yields the single module itself.
        let module_name = |m: &Datum| -> Option<Vec<String>> {
            m.as_list()?
                .iter()
                .map(|part| part.as_symbol().map(str::to_string))
                .collect()
        };
        let modules: Vec<Vec<String>> = match found.assq("modules") {
            Some(one) if module_name(one).is_some() => vec![module_name(one).unwrap()],
            Some(list) => list
                .as_list()
                .map(|l| l.iter().filter_map(module_name).collect())
                .unwrap_or_default(),
            None => Vec::new(),
        };
        let actions: Vec<CodeActionOrCommand> = modules
            .iter()
            .take(6)
            .filter_map(|module| imports::suggest(&snapshot, module))
            .map(|suggestion| {
                CodeActionOrCommand::CodeAction(CodeAction {
                    title: format!("{} for {name}", suggestion.title),
                    kind: Some(CodeActionKind::QUICKFIX),
                    edit: Some(WorkspaceEdit {
                        changes: Some(HashMap::from([(uri.clone(), vec![suggestion.edit])])),
                        ..Default::default()
                    }),
                    ..Default::default()
                })
            })
            .collect();
        Ok((!actions.is_empty()).then_some(actions))
    }

    /// Under Guix, a service type's type definition is its configuration
    /// record; a record constructor leads to its own definition.
    async fn goto_type_definition(
        &self,
        params: GotoTypeDefinitionParams,
    ) -> Result<Option<GotoTypeDefinitionResponse>> {
        let position = params.text_document_position_params;
        let Some(snapshot) = self.snapshot(&position.text_document.uri).await else {
            return Ok(None);
        };
        let Some((_, name)) = semantic::symbol_at(&snapshot, position.position) else {
            return Ok(None);
        };
        let Some(identity) = self.worker.get().and_then(|w| w.identity_if_ready()) else {
            return Ok(None);
        };
        if !adapters::guix::active(&identity) {
            return Ok(None);
        }
        let Some(lookup) = self.lookup(&snapshot, name).await else {
            return Ok(None);
        };
        let Some(module) = adapters::guix::lookup_module(&lookup) else {
            return Ok(None);
        };
        let binding = lookup.assq("binding");
        let kind = binding
            .and_then(|b| b.assq("kind"))
            .and_then(Datum::as_symbol);
        let ty = binding
            .and_then(|b| b.assq("type"))
            .and_then(Datum::as_symbol);
        let exported = binding
            .and_then(|b| b.assq("name"))
            .and_then(Datum::as_symbol)
            .unwrap_or(name)
            .to_string();
        let record = match (kind, ty) {
            (Some("variable"), Some("<service-type>")) => {
                let summary = self
                    .cached(
                        "service-type",
                        adapters::guix::configuration_params(&module, &exported),
                    )
                    .await;
                match summary.as_ref().and_then(adapters::guix::default_record) {
                    Some(record) => record,
                    None => return Ok(None),
                }
            }
            (Some("macro"), _) => exported,
            _ => return Ok(None),
        };
        // The record usually lives in the service type's module; if the
        // document imports its constructor, that module wins.
        let fields = match self.lookup(&snapshot, &record).await {
            Some(found) => {
                let module = adapters::guix::lookup_module(&found).unwrap_or(module);
                self.cached(
                    "configuration",
                    adapters::guix::configuration_params(&module, &record),
                )
                .await
            }
            None => {
                self.cached(
                    "configuration",
                    adapters::guix::configuration_params(&module, &record),
                )
                .await
            }
        };
        Ok(fields
            .filter(adapters::guix::is_record)
            .and_then(|fields| adapters::guix::record_location(&fields))
            .map(GotoTypeDefinitionResponse::Scalar))
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let position = params.text_document_position_params;
        let uri = position.text_document.uri;
        let Some(snapshot) = self.snapshot(&uri).await else {
            return Ok(None);
        };
        if let Some(location) = self.module_location(&snapshot, position.position).await {
            return Ok(Some(GotoDefinitionResponse::Scalar(location)));
        }
        let Some((_, name)) = semantic::symbol_at(&snapshot, position.position) else {
            return Ok(None);
        };
        if let Some((fields, field)) = self.field_at(&snapshot, position.position).await {
            return Ok(
                adapters::guix::field_location(&fields, &field).map(GotoDefinitionResponse::Scalar)
            );
        }
        if let Some(binding) = self.lexical(&snapshot, position.position, name) {
            return Ok(Some(GotoDefinitionResponse::Scalar(Location::new(
                uri,
                snapshot.range(binding.range),
            ))));
        }
        if let Some((defined_in, definition, _)) = self.open_definition(&uri, &snapshot, name).await
        {
            return Ok(Some(GotoDefinitionResponse::Scalar(Location::new(
                defined_in,
                definition.selection_range,
            ))));
        }
        Ok(self
            .lookup(&snapshot, name)
            .await
            .and_then(|lookup| semantic::location_from_lookup(&lookup))
            .map(GotoDefinitionResponse::Scalar))
    }
}

/// Errors located in the validated file become diagnostics; everything else
/// goes into the message.
fn validation_results(
    snapshot: &Snapshot,
    path: &std::path::Path,
    report: &Validation,
) -> (Vec<Diagnostic>, (MessageType, String)) {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut diagnostics = Vec::new();
    let mut lines = Vec::new();
    for error in &report.errors {
        let here = error.file.as_ref().is_some_and(|f| {
            let f = std::path::Path::new(f);
            f == path || std::fs::canonicalize(f).ok().as_deref() == Some(canonical.as_path())
        });
        let mut text = error.message.clone();
        if let Some(hint) = &error.hint {
            text.push_str("\nHint: ");
            text.push_str(hint);
        }
        match (here, error.line) {
            (true, Some(line)) => {
                let range = datum_range(snapshot, line, error.column.unwrap_or(0));
                diagnostics.push(Diagnostic {
                    range,
                    severity: Some(DiagnosticSeverity::ERROR),
                    code: Some(NumberOrString::String("validation".into())),
                    source: Some("guix validation".into()),
                    message: text,
                    ..Default::default()
                });
                lines.push(format!("line {line}: {}", error.message));
            }
            _ => {
                let place = match (&error.file, error.line) {
                    (Some(file), Some(line)) => format!("{file}:{line}: "),
                    (Some(file), None) => format!("{file}: "),
                    _ => String::new(),
                };
                lines.push(format!("{place}{text}"));
            }
        }
    }
    let message = if report.ok() {
        (
            MessageType::INFO,
            format!("{name}: {} ({})", report.summary, report.checks),
        )
    } else {
        (
            MessageType::ERROR,
            format!("{name}: {} error(s). {}", lines.len(), lines.join(" | ")),
        )
    };
    (diagnostics, message)
}

/// The datum starting at Guix's 1-based `line` and 0-based `column`, or the
/// innermost datum around that point, or the line itself.
fn datum_range(snapshot: &Snapshot, line: u32, column: u32) -> Range {
    let position = guile_lsp::syntax::position::Position {
        line: line.saturating_sub(1),
        character: column,
    };
    let Some(offset) = snapshot.lines.offset(&snapshot.text, position) else {
        return Range::new(Position::new(0, 0), Position::new(0, 0));
    };
    let mut best: Option<&guile_lsp::syntax::Node> = None;
    for node in snapshot.tree.root.descendants() {
        if node.kind.is_datum() && node.range.start <= offset && offset < node.range.end {
            best = Some(node);
        }
    }
    match best {
        Some(node) => snapshot.range(node.range.clone()),
        None => {
            let start = snapshot
                .lines
                .line_start(position.line as usize)
                .unwrap_or(0);
            let end = snapshot
                .lines
                .line_start(position.line as usize + 1)
                .map(|n| n - 1)
                .unwrap_or(snapshot.text.len());
            snapshot.range(start..end)
        }
    }
}

#[tokio::main]
async fn main() {
    // Any other arguments (such as `--stdio`) start the server.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("format") {
        std::process::exit(guile_lsp::cli::format_command(&args[1..]));
    }
    // stdout belongs exclusively to the LSP transport.
    let (service, socket) = LspService::new(|client| Backend {
        client,
        documents: RwLock::new(Documents::default()),
        settings: RwLock::new(Settings::default()),
        worker: OnceLock::new(),
        roots: Mutex::new(BTreeSet::new()),
        lookups: Arc::new(Mutex::new(LookupCache::new(4096))),
        warmed: Mutex::new(HashMap::new()),
        analyses: Mutex::new(HashMap::new()),
        semantic: Arc::new(Mutex::new(HashMap::new())),
        restricted_notified: Arc::new(AtomicBool::new(false)),
        dir_locals: Mutex::new(Vec::new()),
        validations: Mutex::new(HashMap::new()),
        validated: Arc::new(Mutex::new(HashMap::new())),
        recent: Mutex::new(None),
        warming: Arc::new(Mutex::new(HashSet::new())),
    });
    Server::new(tokio::io::stdin(), tokio::io::stdout(), socket)
        // Preserve notification order.
        .concurrency_level(1)
        .serve(service)
        .await;
}
