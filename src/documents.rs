use crate::syntax::position::LineIndex;
use crate::syntax::{parse, Tree};
use std::collections::HashMap;
use std::sync::Arc;
use tower_lsp::lsp_types::{DidChangeTextDocumentParams, TextDocumentItem, Url};

/// Immutable contents plus the parsed tree for one document version.
/// Cheap to clone; background jobs can retain one without holding a lock.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub version: i32,
    pub text: Arc<str>,
    pub tree: Arc<Tree>,
    pub lines: Arc<LineIndex>,
}

impl Snapshot {
    fn new(version: i32, text: Arc<str>) -> Self {
        let tree = Arc::new(parse(&text));
        let lines = Arc::new(LineIndex::new(&text));
        Snapshot {
            version,
            text,
            tree,
            lines,
        }
    }
}

#[derive(Default)]
pub struct Documents {
    open: HashMap<Url, Snapshot>,
}

impl Documents {
    pub fn open(&mut self, document: TextDocumentItem) -> Snapshot {
        let snapshot = Snapshot::new(document.version, document.text.into());
        self.open.insert(document.uri, snapshot.clone());
        snapshot
    }

    pub fn get(&self, uri: &Url) -> Option<&Snapshot> {
        self.open.get(uri)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Url, &Snapshot)> {
        self.open.iter()
    }

    pub fn change(
        &mut self,
        params: DidChangeTextDocumentParams,
    ) -> Result<Snapshot, &'static str> {
        let snapshot = self
            .open
            .get_mut(&params.text_document.uri)
            .ok_or("Ignoring change for an unopened document")?;
        if params.text_document.version <= snapshot.version {
            return Err("Ignoring obsolete document version");
        }
        if params.content_changes.is_empty() {
            return Err("Ignoring empty document change");
        }
        // Validate the entire batch before replacing the snapshot.
        if params
            .content_changes
            .iter()
            .any(|change| change.range.is_some())
        {
            return Err("Expected full document synchronization; ranged edits are unsupported");
        }
        let text = params.content_changes.into_iter().last().unwrap().text;
        // Retain the allocation and the tree when only the version changes.
        if snapshot.text.as_ref() != text {
            *snapshot = Snapshot::new(params.text_document.version, text.into());
        } else {
            snapshot.version = params.text_document.version;
        }
        Ok(snapshot.clone())
    }

    pub fn close(&mut self, uri: &Url) {
        self.open.remove(uri);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower_lsp::lsp_types::{
        Position, Range, TextDocumentContentChangeEvent, VersionedTextDocumentIdentifier,
    };

    fn fixture() -> (Documents, Url) {
        let uri = Url::parse("file:///config.scm").unwrap();
        let mut documents = Documents::default();
        documents.open(TextDocumentItem {
            uri: uri.clone(),
            language_id: "scheme".into(),
            version: 1,
            text: "(define name \"λ🦀\")".into(),
        });
        (documents, uri)
    }

    fn change(uri: &Url, version: i32, text: &str) -> DidChangeTextDocumentParams {
        DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier::new(uri.clone(), version),
            content_changes: vec![TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: text.into(),
            }],
        }
    }

    #[test]
    fn replaces_unicode_contents_and_releases_closed_documents() {
        let (mut documents, uri) = fixture();
        let old = documents.get(&uri).unwrap().clone();
        assert!(old.tree.errors.is_empty());
        let new = documents.change(change(&uri, 2, "(unfinished λ")).unwrap();
        assert_eq!(&*new.text, "(unfinished λ");
        assert_eq!(new.tree.errors.len(), 1);
        assert_eq!(&*old.text, "(define name \"λ🦀\")");
        assert!(old.tree.errors.is_empty(), "old snapshot is unaffected");
        documents.close(&uri);
        assert!(documents.get(&uri).is_none());
        assert!(documents.change(change(&uri, 3, "")).is_err());
    }

    #[test]
    fn rejects_obsolete_versions_and_ranged_batches_without_mutating() {
        let (mut documents, uri) = fixture();
        assert!(documents.change(change(&uri, 1, "stale")).is_err());
        let mut invalid = change(&uri, 2, "replacement");
        invalid
            .content_changes
            .push(TextDocumentContentChangeEvent {
                range: Some(Range::new(Position::new(0, 0), Position::new(0, 1))),
                range_length: None,
                text: "(".into(),
            });
        assert!(documents.change(invalid).is_err());
        let snapshot = documents.get(&uri).unwrap();
        assert_eq!(snapshot.version, 1);
        assert_eq!(&*snapshot.text, "(define name \"λ🦀\")");
    }

    #[test]
    fn version_only_changes_keep_the_parsed_tree() {
        let (mut documents, uri) = fixture();
        let before = Arc::as_ptr(&documents.get(&uri).unwrap().tree);
        let after = documents
            .change(change(&uri, 2, "(define name \"λ🦀\")"))
            .unwrap();
        assert_eq!(after.version, 2);
        assert_eq!(Arc::as_ptr(&after.tree), before);
    }
}
