//! Request handling against in-memory state.
//!
//! Everything here is parse, walk and convert against a snapshot. No Bazel:
//! that is invariant 1, expressed as a module boundary. The only filesystem
//! call is a `stat` asking whether a label names a source file, which costs
//! one syscall and cannot block on anything.

use std::path::{Path, PathBuf};

use bls_index::label::{Label, parse_label};
use lsp_types::{
    BaseSymbolInformation, Diagnostic, DiagnosticSeverity, DocumentSymbol, Location, LocationLink,
    Position, Range, SymbolKind, Uri, WorkspaceSymbol,
};
use starlark_cst::ast::{Arg, AstNode, Expr, File, LiteralExpr, LoadItem, LoadStmt, Stmt};
use starlark_cst::{Dialect, FileKind, SyntaxElement, SyntaxKind, SyntaxNode, parse};

use crate::line_index::LineIndex;

/// A target declared in a BUILD file, with the ranges an editor needs.
pub struct Declaration {
    pub name: String,
    pub rule: String,
    /// The whole rule call.
    pub full: Range,
    /// Just the name string's content, quotes excluded.
    pub selection: Range,
}

/// Every target a BUILD file declares.
///
/// Only BUILD files declare targets. `MODULE.bazel` is full of top-level calls
/// carrying a `name` — `bazel_dep(name = "rules_shell")` — and reporting those
/// as targets invents labels that resolve to nothing.
///
/// Legacy macros are invisible here by construction: `legacy_macro(name = "x")`
/// yields `x`, but the `x_0`, `x_1` it actually declares are computed at
/// evaluation time and only Bazel knows them.
#[must_use]
pub fn declarations(text: &str, dialect: Dialect, kind: FileKind) -> Vec<Declaration> {
    if kind != FileKind::Build {
        return Vec::new();
    }
    let lines = LineIndex::new(text);
    let parsed = parse(text, dialect);
    let Some(file) = File::cast(parsed.syntax()) else {
        return Vec::new();
    };

    file.stmts()
        .filter_map(|stmt| match stmt {
            Stmt::Expr(expr) => expr.expr(),
            _ => None,
        })
        .filter_map(|expr| match expr {
            Expr::Call(call) => Some(call),
            _ => None,
        })
        .filter_map(|call| {
            let rule = call.callee_name()?;
            let Expr::Literal(name) = call.arg("name")? else {
                return None;
            };
            let value = name.string_value()?;
            let span = call.range();
            let full = Range {
                start: lines.position(text, usize::from(span.start())),
                end: lines.position(text, usize::from(span.end())),
            };
            let selection = name.string_value_range().map_or(full, |r| Range {
                start: lines.position(text, usize::from(r.start())),
                end: lines.position(text, usize::from(r.end())),
            });
            Some(Declaration {
                name: value,
                rule,
                full,
                selection,
            })
        })
        .collect()
}

#[must_use]
pub fn document_symbols(text: &str, dialect: Dialect, kind: FileKind) -> Vec<DocumentSymbol> {
    declarations(text, dialect, kind)
        .into_iter()
        .map(|d| DocumentSymbol {
            name: format!(":{}", d.name),
            kind: symbol_kind(&d.rule),
            detail: Some(d.rule),
            tags: None,
            #[allow(deprecated)]
            deprecated: None,
            range: d.full,
            selection_range: d.selection,
            children: None,
        })
        .collect()
}

/// Syntax errors, as diagnostics.
///
/// The parser always returns a tree, so this never suppresses other features;
/// a file with errors still yields whatever symbols survived recovery.
#[must_use]
pub fn syntax_diagnostics(text: &str, dialect: Dialect) -> Vec<Diagnostic> {
    let lines = LineIndex::new(text);
    parse(text, dialect)
        .errors()
        .iter()
        .map(|error| Diagnostic {
            range: Range {
                start: lines.position(text, usize::from(error.range.start())),
                end: lines.position(text, usize::from(error.range.end())),
            },
            severity: Some(DiagnosticSeverity::Error),
            source: Some("bazel-language-server".to_string()),
            message: error.message.clone().into(),
            ..Default::default()
        })
        .collect()
}

/// A `SymbolKind` chosen so a picker's kind column says something.
///
/// LSP has no kind for "build target", so every target sharing one renders as a
/// column of identical `[Object]` — noise in a list of hundreds. Grouping by
/// what the rule *does* makes tests and binaries findable at a glance. The rule
/// name is still carried exactly, in `containerName`.
fn symbol_kind(rule: &str) -> SymbolKind {
    match rule {
        r if r.ends_with("_test") || r == "test_suite" => SymbolKind::Event,
        r if r.ends_with("_binary") => SymbolKind::Function,
        r if r.ends_with("_library") || r.ends_with("_module") => SymbolKind::Module,
        "alias" => SymbolKind::Interface,
        "filegroup" | "exports_files" | "pkg_files" => SymbolKind::File,
        "genrule" | "run_binary" => SymbolKind::Constructor,
        r if r.ends_with("_setting") || r.ends_with("_flag") => SymbolKind::Constant,
        _ => SymbolKind::Struct,
    }
}

/// Workspace symbols from the static index.
///
/// Undercounts until the graph tier lands, which is why the caller must not
/// present this as exhaustive. See `ROADMAP.md` G4.
#[must_use]
pub fn workspace_symbols(index: &bls_index::Index, query: &str) -> Vec<WorkspaceSymbol> {
    let needle = query.to_lowercase();
    index
        .targets
        .iter()
        .filter(|(label, _)| needle.is_empty() || label.to_lowercase().contains(&needle))
        .take(512)
        .filter_map(|(label, target)| {
            let uri = file_uri(index.path(target.file)?)?;
            let at = Position {
                line: target.line,
                character: target.character,
            };
            Some(WorkspaceSymbol {
                location: Location {
                    uri,
                    range: Range { start: at, end: at },
                }
                .into(),
                data: None,
                base_symbol_information: BaseSymbolInformation {
                    name: label.clone(),
                    kind: symbol_kind(&target.rule),
                    tags: None,
                    container_name: Some(target.rule.to_string()),
                },
            })
        })
        .collect()
}

fn file_uri(path: &Path) -> Option<Uri> {
    format!("file://{}", path.display()).parse().ok()
}

/// What a string in a build file refers to, decided by where it sits.
#[derive(Debug)]
enum StringRole {
    /// The first item of a `load()`: the `.bzl` file being loaded from.
    LoadModule,
    /// A symbol in a `load()`, carrying the module it comes from.
    ///
    /// Jumping to the symbol's own `def` would mean parsing the module and
    /// resolving the name in it, which is a second file and a second index;
    /// this jumps to the file that defines it instead. Landing in the right
    /// file is most of the value and none of the risk.
    LoadSymbol(String),
    /// The `name` of a top-level rule call: the target being declared rather
    /// than a label pointing at one. `name = "srcs"` in `//lib` is `//lib:srcs`
    /// and nothing else, where a label of the same text would first have to be
    /// resolved against the package that wrote it.
    TargetName,
    /// Anything else. In a build file that means a label.
    Label,
}

/// A string literal's content and what it is for.
#[derive(Debug)]
struct StringAt {
    value: String,
    /// Byte range of the content, quotes and prefix excluded.
    range: std::ops::Range<u32>,
    role: StringRole,
}

/// The string literal whose content contains `offset`.
///
/// `None` when the cursor is anywhere else — on a quote, an identifier, or in
/// the whitespace between them.
fn string_at(root: &SyntaxNode, offset: u32) -> Option<StringAt> {
    // rowan's `token_at_offset` wants a `TextSize`, which starlark-cst does not
    // re-export. A scan is O(tokens) against a parse that is O(bytes) and has
    // just happened anyway, so it is not the cost that matters here.
    let token = root
        .descendants_with_tokens()
        .filter_map(SyntaxElement::into_token)
        .find(|token| {
            token.kind() == SyntaxKind::STRING
                && (u32::from(token.text_range().start())..u32::from(token.text_range().end()))
                    .contains(&offset)
        })?;
    let parent = token.parent()?;

    // A `load()` item holds its string token directly; every other string is
    // wrapped in a literal expression.
    let (value, range, role) = if let Some(item) = LoadItem::cast(parent.clone()) {
        let load = LoadStmt::cast(parent.parent()?)?;
        let module = load.module()?;
        let role = if module == item {
            StringRole::LoadModule
        } else {
            StringRole::LoadSymbol(module.value()?)
        };
        (item.value()?, item.value_range()?, role)
    } else {
        let literal = LiteralExpr::cast(parent)?;
        let role = if declares_a_target(&literal) {
            StringRole::TargetName
        } else {
            StringRole::Label
        };
        (literal.string_value()?, literal.string_value_range()?, role)
    };

    let range = u32::from(range.start())..u32::from(range.end());
    // The content range excludes the quotes, so a cursor on one is not on the
    // label. Its far end counts: that is where the caret sits after typing.
    (range.start..=range.end)
        .contains(&offset)
        .then_some(StringAt { value, range, role })
}

/// Whether a string is the `name` of a rule call at the top level of the file.
///
/// Only the top level declares targets: `name = "x"` inside a `select()` or a
/// nested call is an argument that happens to be called `name`, and treating it
/// as a declaration would attribute every reference to the wrong label.
fn declares_a_target(literal: &LiteralExpr) -> bool {
    let Some(arg) = literal.syntax().parent().and_then(Arg::cast) else {
        return false;
    };
    if arg.name().as_deref() != Some("name") {
        return false;
    }
    // ARG -> ARG_LIST -> CALL_EXPR -> EXPR_STMT -> FILE
    arg.syntax()
        .parent()
        .and_then(|list| list.parent())
        .filter(|call| call.kind() == SyntaxKind::CALL_EXPR)
        .and_then(|call| call.parent())
        .filter(|stmt| stmt.kind() == SyntaxKind::EXPR_STMT)
        .and_then(|stmt| stmt.parent())
        .is_some_and(|file| file.kind() == SyntaxKind::FILE)
}

/// The package a file belongs to, as a workspace-relative directory.
///
/// The package is the nearest ancestor holding a BUILD file, not simply the
/// file's own directory: `app/nested/data.txt` belongs to `//app`, and calling
/// it `//app/nested` would resolve every relative label in it to a package
/// that does not exist.
///
/// `None` when no ancestor up to the workspace root has a BUILD file. Relative
/// labels are then unresolvable, which is correct — there is no package for
/// them to be relative to — while `//pkg:target` still works.
fn enclosing_package(root: &Path, file: &Path) -> Option<String> {
    if !file.starts_with(root) {
        return None;
    }
    let mut dir = file.parent()?;
    loop {
        if dir.join("BUILD.bazel").is_file() || dir.join("BUILD").is_file() {
            return Some(dir.strip_prefix(root).ok()?.to_str()?.replace('\\', "/"));
        }
        if dir == root {
            return None;
        }
        dir = dir.parent()?;
    }
}

/// Where a definition lives, and the position to reveal in it.
struct Site {
    path: PathBuf,
    at: Position,
}

/// The declaring rule call, from the index snapshot.
fn target_site(index: &bls_index::Index, label: &Label) -> Option<Site> {
    let target = index.target(&label.key())?;
    Some(Site {
        path: index.path(target.file)?.to_path_buf(),
        // The index carries where the name starts and not where the call ends,
        // so the range is empty. Clients reveal the line either way, and
        // re-reading the file to widen it would put IO in the request path.
        at: Position {
            line: target.line,
            character: target.character,
        },
    })
}

/// The source file a label names, for the `srcs = ["main.sh"]` case.
///
/// A source file is a target in its own right, so this is a definition and not
/// a consolation prize. It is tried after the index because a rule and a source
/// file cannot share a name, and the rule is what a label with that name means.
fn file_site(root: &Path, label: &Label) -> Option<Site> {
    let path = root.join(label.path());
    path.is_file().then_some(Site {
        path,
        at: Position {
            line: 0,
            character: 0,
        },
    })
}

/// Goto-definition for the string under the cursor.
///
/// A string is read as a `load()` path, a symbol in a `load()`, or a label,
/// decided by where it sits. A label resolves to the declaring rule call in the
/// index, or failing that to the source file it names; a `load()` resolves to
/// the `.bzl` file, and so does a symbol inside one — following the symbol to
/// its own `def` is out of scope.
///
/// Main repo only. Everything is answered from the index snapshot and the
/// document text; nothing here can invoke Bazel, and an unresolvable label
/// yields nothing rather than a guess.
#[must_use]
pub fn definition(
    text: &str,
    dialect: Dialect,
    file: &Path,
    root: &Path,
    index: &bls_index::Index,
    position: Position,
) -> Vec<LocationLink> {
    let lines = LineIndex::new(text);
    let Ok(offset) = u32::try_from(lines.offset(text, position)) else {
        return Vec::new();
    };
    let Some(found) = string_at(&parse(text, dialect).syntax(), offset) else {
        return Vec::new();
    };

    let package = enclosing_package(root, file);
    let site = match &found.role {
        // A load path names a file, never a target, so the index is not
        // consulted: a rule that happened to be called `defs.bzl` is not it.
        StringRole::LoadModule => {
            parse_label(&found.value, package.as_deref()).and_then(|label| file_site(root, &label))
        }
        StringRole::LoadSymbol(module) => {
            parse_label(module, package.as_deref()).and_then(|label| file_site(root, &label))
        }
        // The cursor is on the declaration already, so there is nowhere to go.
        // Jumping to the line it is sitting on reads as the server having
        // failed. The variant earns its keep in `references`.
        StringRole::TargetName => None,
        StringRole::Label => parse_label(&found.value, package.as_deref()).and_then(|label| {
            target_site(index, &label)
                .or_else(|| file_site(root, &label))
                .or_else(|| {
                    tracing::debug!(
                        label = label.key(),
                        "no such target in the static index and no source file at its path; \
                         legacy macros and external repositories need the graph tier"
                    );
                    None
                })
        }),
    };

    let Some(site) = site else {
        return Vec::new();
    };
    let Some(uri) = file_uri(&site.path) else {
        return Vec::new();
    };
    let target = Range {
        start: site.at,
        end: site.at,
    };
    vec![LocationLink {
        // Highlight the label text alone, without its quotes.
        origin_selection_range: Some(Range {
            start: lines.position(text, found.range.start as usize),
            end: lines.position(text, found.range.end as usize),
        }),
        target_uri: uri,
        target_range: target,
        target_selection_range: target,
    }]
}

/// Every place in the main repository that names the target under the cursor.
///
/// The cursor may be on a label (`"//lib:srcs"`, `":srcs"`) or on the `name` of
/// the rule declaring it; both resolve to the same target.
///
/// **Partial by construction, in two ways a caller must not paper over.**
/// External repositories are not searched, because resolving `@repo//…` needs
/// the repo mapping only Bazel can produce. And the static tier cannot see
/// targets or references that legacy macros compute at evaluation time — a
/// macro emitting `deps = [name + "_lib"]` is invisible here. Both wait on the
/// graph tier; see `ROADMAP.md` G4.
#[must_use]
pub fn references(
    text: &str,
    dialect: Dialect,
    file: &Path,
    root: &Path,
    index: &bls_index::Index,
    position: Position,
    include_declaration: bool,
) -> Vec<Location> {
    let lines = LineIndex::new(text);
    let Ok(offset) = u32::try_from(lines.offset(text, position)) else {
        return Vec::new();
    };
    let Some(found) = string_at(&parse(text, dialect).syntax(), offset) else {
        return Vec::new();
    };

    let package = enclosing_package(root, file);
    let key = match &found.role {
        // A declaration names its own package; a label has to be resolved
        // against the package that wrote it.
        StringRole::TargetName => match package.as_deref() {
            Some(package) => Label::new(package, &found.value).key(),
            None => return Vec::new(),
        },
        StringRole::Label => match parse_label(&found.value, package.as_deref()) {
            Some(label) => label.key(),
            None => return Vec::new(),
        },
        // A `.bzl` file is not a target, so it has no referring labels. Finding
        // the files that `load()` it is a different question, and answering
        // this one with those would be a wrong answer rather than none.
        StringRole::LoadModule | StringRole::LoadSymbol(_) => return Vec::new(),
    };

    let mut sites: Vec<(PathBuf, Position)> = index
        .references(&key)
        .iter()
        .filter_map(|reference| {
            Some((
                index.path(reference.file)?.to_path_buf(),
                Position {
                    line: reference.line,
                    character: reference.character,
                },
            ))
        })
        .collect();

    if include_declaration && let Some(target) = index.target(&key) {
        if let Some(path) = index.path(target.file) {
            sites.push((
                path.to_path_buf(),
                Position {
                    line: target.line,
                    character: target.character,
                },
            ));
        }
    }

    // Stable between calls, so a picker does not reshuffle under the user.
    sites.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(a.1.line.cmp(&b.1.line))
            .then(a.1.character.cmp(&b.1.character))
    });
    sites.dedup();

    tracing::debug!(label = key, count = sites.len(), "references");
    sites
        .into_iter()
        .filter_map(|(path, at)| {
            Some(Location {
                uri: file_uri(&path)?,
                range: Range { start: at, end: at },
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUILD: &str = "\
filegroup(\n    name = \"srcs\",\n    srcs = [],\n)\n\ncc_library(name = \"core\")\n";

    /// `name` identifies a declaration only at the top level. Nested, it is an
    /// argument that happens to share the spelling, and counting it as a
    /// declaration would hang every reference off the wrong label.
    #[test]
    fn only_a_top_level_name_declares_a_target() {
        let role = |text: &str, needle: &str| {
            let offset = u32::try_from(text.find(needle).expect("needle") + 1).unwrap();
            string_at(&parse(text, Dialect::Bazel).syntax(), offset).map(|found| found.role)
        };

        assert!(matches!(
            role("filegroup(name = \"srcs\")\n", "srcs"),
            Some(StringRole::TargetName)
        ));
        // An argument called `name` on a call nested inside another.
        assert!(matches!(
            role("filegroup(srcs = glob(name = \"inner\"))\n", "inner"),
            Some(StringRole::Label)
        ));
        // Any other attribute, at the top level.
        assert!(matches!(
            role("filegroup(srcs = [\"//lib:a\"])\n", "//lib:a"),
            Some(StringRole::Label)
        ));
    }

    /// Goto-definition on a declaration must not jump to the line the cursor is
    /// already on; that reads as the server having misfired.
    #[test]
    fn definition_on_a_declaration_goes_nowhere() {
        let fixture = Fixture::torture();
        let file = fixture.root.join("lib/BUILD.bazel");
        let text = std::fs::read_to_string(&file).expect("fixture");
        let offset = text.find("\"srcs\"").expect("the srcs declaration") + 2;
        let lines = LineIndex::new(&text);

        let jumps = definition(
            &text,
            Dialect::Bazel,
            &file,
            &fixture.root,
            &bls_index::Index::default(),
            lines.position(&text, offset),
        );
        assert!(jumps.is_empty(), "got {jumps:?}");
    }

    /// From the declaration and from a label pointing at it, the answer is the
    /// same target — and `includeDeclaration` is what decides whether the
    /// declaring line is in it.
    #[test]
    fn references_agree_from_either_end() {
        let fixture = Fixture::torture();
        let index = bls_index::build_static(&fixture.root);
        let file = fixture.root.join("lib/BUILD.bazel");
        let text = std::fs::read_to_string(&file).expect("fixture");
        let lines = LineIndex::new(&text);

        let at = |needle: &str, skip: usize| {
            let offset = text.find(needle).expect("needle") + skip;
            references(
                &text,
                Dialect::Bazel,
                &file,
                &fixture.root,
                &index,
                lines.position(&text, offset),
                false,
            )
        };

        // `//lib:srcs` is referenced by `:srcs` from the alias, the genrule,
        // a select() branch and a rule attribute.
        let from_declaration = at("\"srcs\"", 2);
        assert_eq!(
            from_declaration.len(),
            4,
            "expected every referrer, got {from_declaration:?}"
        );

        // From a label pointing at it: `actual = ":srcs"`.
        let from_label = at("actual = \":srcs\"", 11);
        assert_eq!(
            from_label, from_declaration,
            "a label and its declaration name the same target"
        );

        // The declaration is a separate site, added only when asked for.
        let offset = text.find("\"srcs\"").expect("needle") + 2;
        let with_declaration = references(
            &text,
            Dialect::Bazel,
            &file,
            &fixture.root,
            &index,
            lines.position(&text, offset),
            true,
        );
        assert_eq!(with_declaration.len(), from_declaration.len() + 1);
    }

    /// A `.bzl` path is not a target. Answering with the files that `load()` it
    /// would be a different question answered wrongly.
    #[test]
    fn references_of_a_load_path_are_empty() {
        let fixture = Fixture::torture();
        let index = bls_index::build_static(&fixture.root);
        let file = fixture.root.join("lib/BUILD.bazel");
        let text = std::fs::read_to_string(&file).expect("fixture");
        let lines = LineIndex::new(&text);
        let offset = text.find("//macros:legacy.bzl").expect("a load path") + 4;

        assert!(
            references(
                &text,
                Dialect::Bazel,
                &file,
                &fixture.root,
                &index,
                lines.position(&text, offset),
                true,
            )
            .is_empty()
        );
    }

    #[test]
    fn finds_every_declaration() {
        let found = declarations(BUILD, Dialect::Bazel, FileKind::Build);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].name, "srcs");
        assert_eq!(found[0].rule, "filegroup");
        assert_eq!(found[1].name, "core");

        // The selection range covers the name only, not the whole call.
        assert_eq!(found[0].selection.start.line, 1);
        assert!(found[0].selection.start.character > 0);
    }

    #[test]
    fn symbols_are_prefixed_like_labels() {
        let symbols = document_symbols(BUILD, Dialect::Bazel, FileKind::Build);
        assert_eq!(symbols[0].name, ":srcs");
        assert_eq!(symbols[0].detail.as_deref(), Some("filegroup"));
    }

    #[test]
    fn module_bazel_declares_no_targets() {
        let module = "bazel_dep(name = \"rules_shell\", version = \"0.3.0\")\n";
        assert!(declarations(module, Dialect::Bazel, FileKind::Module).is_empty());
        // The same text read as a BUILD file would look like a target.
        assert_eq!(
            declarations(module, Dialect::Bazel, FileKind::Build).len(),
            1
        );
    }

    #[test]
    fn broken_input_still_yields_symbols() {
        let broken = "filegroup(name = \"a\",\n\ncc_library(name = \"b\")\n";
        assert!(!syntax_diagnostics(broken, Dialect::Bazel).is_empty());
        // Recovery is local, so the file is not written off entirely.
        assert!(
            !parse(broken, Dialect::Bazel)
                .syntax()
                .text()
                .to_string()
                .is_empty()
        );
    }

    fn torture() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../experiments/torture")
            .canonicalize()
            .expect("the torture workspace is checked in")
    }

    #[test]
    fn a_package_is_the_nearest_build_file() {
        let root = torture();
        assert_eq!(
            enclosing_package(&root, &root.join("lib/BUILD.bazel")).as_deref(),
            Some("lib")
        );
        assert_eq!(
            enclosing_package(&root, &root.join("lib/sub/BUILD.bazel")).as_deref(),
            Some("lib/sub")
        );
        // `app/nested` has no BUILD file, so its contents belong to `//app`.
        assert_eq!(
            enclosing_package(&root, &root.join("app/nested/data.txt")).as_deref(),
            Some("app")
        );
        // The torture root has a MODULE.bazel and no BUILD file, so it is not
        // a package at all.
        assert_eq!(enclosing_package(&root, &root.join("MODULE.bazel")), None);
    }

    /// Drives `definition` the way the server does: read the file, put the
    /// cursor in the middle of `needle`, and report where it lands.
    struct Fixture {
        root: PathBuf,
        index: bls_index::Index,
    }

    impl Fixture {
        fn torture() -> Self {
            let root = torture();
            let index = bls_index::build_static(&root);
            Self { root, index }
        }

        fn links(&self, relative: &str, needle: &str) -> Vec<LocationLink> {
            let file = self.root.join(relative);
            let text = std::fs::read_to_string(&file).expect("fixture file");
            let at = text.find(needle).unwrap_or_else(|| {
                panic!("{needle:?} is not in {relative}");
            }) + needle.len() / 2;
            let lines = LineIndex::new(&text);
            definition(
                &text,
                Dialect::Bazel,
                &file,
                &self.root,
                &self.index,
                lines.position(&text, at),
            )
        }

        /// Where the cursor lands, as `path:line:character` relative to the
        /// workspace root.
        fn go(&self, relative: &str, needle: &str) -> Option<String> {
            let link = self.links(relative, needle).into_iter().next()?;
            let path = link.target_uri.path().as_str().to_string();
            let path = path
                .strip_prefix(self.root.to_str().unwrap())?
                .trim_start_matches('/')
                .to_string();
            Some(format!(
                "{path}:{}:{}",
                link.target_range.start.line, link.target_range.start.character
            ))
        }
    }

    #[test]
    fn labels_resolve_through_the_index() {
        let fixture = Fixture::torture();

        // An absolute label into another package.
        assert_eq!(
            fixture
                .go("lib/BUILD.bazel", "//lib/sub:sub_srcs")
                .as_deref(),
            Some("lib/sub/BUILD.bazel:3:12")
        );
        // A relative one, against the package the file is in.
        assert_eq!(
            fixture.go("lib/BUILD.bazel", ":aliased").as_deref(),
            Some("lib/BUILD.bazel:34:12")
        );
        // A label pointing back up out of a subpackage.
        assert_eq!(
            fixture
                .go("lib/sub/BUILD.bazel", "//lib:exported.txt")
                .as_deref(),
            Some("lib/exported.txt:0:0")
        );
    }

    /// `srcs = ["tool.sh"]` names a source file, and a source file is a target.
    #[test]
    fn source_files_are_definitions() {
        let fixture = Fixture::torture();
        assert_eq!(
            fixture.go("app/BUILD.bazel", "tool.sh").as_deref(),
            Some("app/tool.sh:0:0")
        );
        // A file in a subdirectory of the package that owns it. The quotes are
        // part of the needle because a comment above names the same label, and
        // a cursor in a comment is a cursor in no string at all.
        assert_eq!(
            fixture
                .go("lib/BUILD.bazel", "\"//app:nested/data.txt\"")
                .as_deref(),
            Some("app/nested/data.txt:0:0")
        );
    }

    #[test]
    fn load_paths_and_their_symbols_reach_the_file() {
        let fixture = Fixture::torture();
        assert_eq!(
            fixture
                .go("lib/BUILD.bazel", "//macros:legacy.bzl")
                .as_deref(),
            Some("macros/legacy.bzl:0:0")
        );
        // The symbol jumps to the file that defines it, not to the `def`.
        assert_eq!(
            fixture.go("lib/BUILD.bazel", "legacy_macro").as_deref(),
            Some("macros/legacy.bzl:0:0")
        );
        // A load path relative to the current package.
        assert_eq!(
            fixture.go("lib/BUILD.bazel", ":local.bzl").as_deref(),
            Some("lib/local.bzl:0:0")
        );
        // An aliased symbol: the string is the original name, and the alias
        // token beside it is not a string at all.
        assert_eq!(
            fixture.go("lib/BUILD.bazel", "renamed_in_load").as_deref(),
            Some("lib/local.bzl:0:0")
        );
    }

    #[test]
    fn external_labels_yield_nothing() {
        let fixture = Fixture::torture();
        assert!(
            fixture
                .links("lib/BUILD.bazel", "@platforms//os:linux")
                .is_empty()
        );
        assert!(
            fixture
                .links("lib/BUILD.bazel", "@bazel_skylib//rules:write_file.bzl")
                .is_empty()
        );
    }

    #[test]
    fn a_label_naming_nothing_yields_nothing() {
        let fixture = Fixture::torture();
        // The torture workspace has this deliberately dangling label.
        assert!(
            fixture
                .links("lib/sub/BUILD.bazel", "//lib:does_not_exist")
                .is_empty()
        );
        // And these are pseudo-labels that never name a target.
        assert!(
            fixture
                .links("lib/BUILD.bazel", "//visibility:public")
                .is_empty()
        );
        assert!(
            fixture
                .links("lib/BUILD.bazel", "//conditions:default")
                .is_empty()
        );
    }

    #[test]
    fn a_cursor_outside_a_string_yields_nothing() {
        let text = "filegroup(\n    name = \"srcs\",\n    srcs = [\"//lib:a\"],\n)\n";
        let lines = LineIndex::new(text);
        let root = torture();
        let index = bls_index::Index::default();

        for needle in ["filegroup", "name", "srcs = [", ")"] {
            let at = text.find(needle).unwrap();
            let found = definition(
                text,
                Dialect::Bazel,
                &root.join("lib/BUILD.bazel"),
                &root,
                &index,
                lines.position(text, at),
            );
            assert!(found.is_empty(), "cursor on {needle:?} found {found:?}");
        }

        // The quotes are not part of the label either.
        let quote = text.find("\"//lib:a\"").unwrap();
        assert!(
            definition(
                text,
                Dialect::Bazel,
                &root.join("lib/BUILD.bazel"),
                &root,
                &index,
                lines.position(text, quote),
            )
            .is_empty()
        );
    }

    /// The origin range is what the editor underlines. It has to be the label
    /// alone: including the quotes highlights punctuation the user did not
    /// point at.
    #[test]
    fn the_origin_range_is_the_label_without_its_quotes() {
        let fixture = Fixture::torture();
        let link = fixture
            .links("lib/BUILD.bazel", "//lib/sub:sub_srcs")
            .into_iter()
            .next()
            .expect("the label resolves");
        let text = std::fs::read_to_string(fixture.root.join("lib/BUILD.bazel")).unwrap();
        let lines = LineIndex::new(&text);
        let origin = link.origin_selection_range.expect("an origin range");
        let start = lines.offset(&text, origin.start);
        let end = lines.offset(&text, origin.end);
        assert_eq!(&text[start..end], "//lib/sub:sub_srcs");
    }
}
