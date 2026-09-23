//! Load-path roots inferred from files: a module `(a b c)` living at
//! `/root/a/b/c.scm` implies `/root` is a load-path entry. Workspace folders
//! count as roots too. Roots feed the worker's load path so local modules
//! resolve without configuration.

use crate::documents::Snapshot;
use crate::syntax::Kind;
use std::path::{Path, PathBuf};
use tower_lsp::lsp_types::Url;

/// The `define-module` name declared at the top of a document, if any.
pub fn declared_module(snapshot: &Snapshot) -> Option<Vec<String>> {
    let source = &snapshot.text;
    snapshot.tree.root.data().find_map(|form| {
        let mut items = form.data();
        let head = items.next()?;
        if head.kind != Kind::Symbol || head.text(source) != "define-module" {
            return None;
        }
        let name = items.next()?;
        (name.kind == Kind::List)
            .then(|| {
                name.data()
                    .map(|n| (n.kind == Kind::Symbol).then(|| n.text(source).to_string()))
                    .collect::<Option<Vec<_>>>()
            })
            .flatten()
    })
}

/// The directory that must be on the load path for `module` to be found at
/// `file`: strip `a/b/c.scm` from the end of the path, if it matches.
pub fn module_root(file: &Path, module: &[String]) -> Option<PathBuf> {
    let (last, parents) = module.split_last()?;
    let stem = file.file_stem()?.to_str()?;
    if stem != last || file.extension()?.to_str()? != "scm" {
        return None;
    }
    let mut dir = file.parent()?;
    for component in parents.iter().rev() {
        if dir.file_name()?.to_str()? != component {
            return None;
        }
        dir = dir.parent()?;
    }
    Some(dir.to_path_buf())
}

/// Roots implied by what a document imports: for `(use-modules (my services))`
/// in `/w/config.scm`, the nearest ancestor of `/w` holding
/// `my/services.scm`. Modules found nowhere nearby (Guile's and Guix's own)
/// imply nothing.
pub fn import_roots(file: &Path, imports: &[Vec<String>]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for module in imports {
        let Some((last, parents)) = module.split_last() else {
            continue;
        };
        let mut relative = PathBuf::new();
        for part in parents {
            relative.push(part);
        }
        relative.push(format!("{last}.scm"));
        let mut dir = file.parent();
        while let Some(d) = dir {
            if d.join(&relative).is_file() {
                let root = d.to_path_buf();
                if !out.contains(&root) {
                    out.push(root);
                }
                break;
            }
            dir = d.parent();
        }
    }
    out
}

/// Root implied by an open document, or `None` when it declares no module
/// or its path does not match the declaration.
pub fn root_for(uri: &Url, snapshot: &Snapshot) -> Option<PathBuf> {
    let file = uri.to_file_path().ok()?;
    let module = declared_module(snapshot)?;
    module_root(&file, &module)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::documents::Documents;
    use tower_lsp::lsp_types::TextDocumentItem;

    fn snapshot(uri: &str, text: &str) -> (Url, Snapshot) {
        let uri = Url::parse(uri).unwrap();
        let snapshot = Documents::default().open(TextDocumentItem {
            uri: uri.clone(),
            language_id: "scheme".into(),
            version: 1,
            text: text.into(),
        });
        (uri, snapshot)
    }

    #[test]
    fn infers_roots_from_module_declarations() {
        let (uri, snap) = snapshot(
            "file:///src/app/lib/util.scm",
            "(define-module (app lib util) #:export (x))\n(define x 1)",
        );
        assert_eq!(
            declared_module(&snap),
            Some(vec!["app".into(), "lib".into(), "util".into()])
        );
        assert_eq!(root_for(&uri, &snap), Some(PathBuf::from("/src")));

        let (uri, snap) = snapshot(
            "file:///elsewhere/util.scm",
            "(define-module (app lib util))",
        );
        assert_eq!(
            root_for(&uri, &snap),
            None,
            "path does not match the declaration"
        );

        let (uri, snap) = snapshot(
            "file:///src/script.scm",
            "(use-modules (srfi srfi-1))\n(display 1)",
        );
        assert_eq!(root_for(&uri, &snap), None, "no module declared");

        let (uri, snap) = snapshot("file:///src/top.scm", "(define-module (top))");
        assert_eq!(root_for(&uri, &snap), Some(PathBuf::from("/src")));
    }

    #[test]
    fn imports_imply_roots_when_the_module_file_is_nearby() {
        let file = std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/fixtures/guix/system.scm"
        ));
        let imports = vec![
            vec!["gnu".to_string()],
            vec!["my".to_string(), "services".to_string()],
        ];
        let roots = import_roots(file, &imports);
        assert_eq!(
            roots,
            vec![std::path::PathBuf::from(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/fixtures/guix"
            ))]
        );
    }
}
