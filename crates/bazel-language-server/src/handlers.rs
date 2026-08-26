//! Request handling against in-memory state.
//!
//! Everything here is parse, walk and convert against a snapshot. No Bazel:
//! that is invariant 1, expressed as a module boundary. The only filesystem
//! call is a `stat` asking whether a label names a source file, which costs
//! one syscall and cannot block on anything.

use std::path::{Path, PathBuf};

use lsp_types::{
    BaseSymbolInformation, Diagnostic, DiagnosticSeverity, DocumentSymbol, Location, LocationLink,
    Position, Range, SymbolKind, Uri, WorkspaceSymbol,
};
use starlark_cst::ast::{AstNode, Expr, File, LiteralExpr, LoadItem, LoadStmt, Stmt};
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
        (
            literal.string_value()?,
            literal.string_value_range()?,
            StringRole::Label,
        )
    };

    let range = u32::from(range.start())..u32::from(range.end());
    // The content range excludes the quotes, so a cursor on one is not on the
    // label. Its far end counts: that is where the caret sits after typing.
    (range.start..=range.end)
        .contains(&offset)
        .then_some(StringAt { value, range, role })
}

/// A label resolved against the package that names it.
#[derive(Debug)]
struct Label {
    /// Workspace-relative package directory. Empty for the root package.
    package: String,
    /// The target name. For a source file this is a package-relative path,
    /// which is why it may contain slashes.
    name: String,
}

impl Label {
    /// The index key: `//pkg:name`, and `//:name` at the root.
    fn key(&self) -> String {
        format!("//{}:{}", self.package, self.name)
    }

    /// Where a source file of this name would sit, relative to the workspace.
    fn path(&self) -> PathBuf {
        Path::new(&self.package).join(&self.name)
    }
}

/// Normalise a label written in `package` to its absolute form.
///
/// Handled: `//pkg:target`, `//:target`, `//pkg` (shorthand for
/// `//pkg:<last component>`), `:target`, and a bare `target`. `@//pkg:target`
/// and `@@//pkg:target` name the main repository explicitly and are the same
/// thing.
///
/// Refused, because a guess here is worse than nothing: any other `@repo//` or
/// `@@canonical//` label, which needs the repo mapping only Bazel can produce,
/// and every target *pattern* — `...`, `//pkg:all`, `//pkg:*` — which names a
/// set rather than a target.
fn parse_label(raw: &str, package: Option<&str>) -> Option<Label> {
    let raw = if let Some(rest) = raw.strip_prefix("@@").or_else(|| raw.strip_prefix('@')) {
        if !rest.starts_with("//") {
            tracing::debug!(
                label = raw,
                "external repository: the apparent name maps to a canonical one only Bazel knows, \
                 and the repo may not be fetched at all"
            );
            return None;
        }
        rest
    } else {
        raw
    };

    if raw.contains("...") {
        tracing::debug!(label = raw, "target pattern, not a label");
        return None;
    }

    let (package, name) = if let Some(rest) = raw.strip_prefix("//") {
        match rest.split_once(':') {
            Some((package, name)) => (package, name),
            // `//pkg` means `//pkg:pkg`, taking the last component.
            None => (rest, rest.rsplit('/').next()?),
        }
    } else if let Some(name) = raw.strip_prefix(':') {
        (package?, name)
    } else {
        // A bare name is relative to the current package. A colon anywhere but
        // the front does not make a label at all: `pkg:target` is not one.
        if raw.contains(':') {
            return None;
        }
        (package?, raw)
    };

    if name.is_empty() || package.starts_with('/') || package.ends_with('/') {
        return None;
    }
    if matches!(name, "all" | "*" | "all-targets") {
        tracing::debug!(label = raw, "target pattern, not a label");
        return None;
    }
    // Bazel allows a wide alphabet in target names but no whitespace, so this
    // rejects prose that merely happens to sit in a string.
    if raw.chars().any(char::is_whitespace) {
        return None;
    }

    Some(Label {
        package: package.to_string(),
        name: name.to_string(),
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    const BUILD: &str = "\
filegroup(\n    name = \"srcs\",\n    srcs = [],\n)\n\ncc_library(name = \"core\")\n";

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

    fn key(raw: &str, package: Option<&str>) -> Option<String> {
        parse_label(raw, package).map(|label| label.key())
    }

    #[test]
    fn absolute_labels_are_already_keys() {
        assert_eq!(key("//lib:srcs", None).as_deref(), Some("//lib:srcs"));
        assert_eq!(
            key("//lib/sub:sub_srcs", None).as_deref(),
            Some("//lib/sub:sub_srcs")
        );
        // The root package: the index writes it `//:name`, not `//name`.
        assert_eq!(key("//:beacon", None).as_deref(), Some("//:beacon"));
        // A source file in a subdirectory of its package keeps the slashes.
        assert_eq!(
            key("//app:nested/data.txt", None).as_deref(),
            Some("//app:nested/data.txt")
        );
    }

    #[test]
    fn a_package_alone_names_its_last_component() {
        assert_eq!(
            key("//lib/config", None).as_deref(),
            Some("//lib/config:config")
        );
        assert_eq!(key("//lib", None).as_deref(), Some("//lib:lib"));
    }

    #[test]
    fn relative_labels_take_the_enclosing_package() {
        assert_eq!(key(":srcs", Some("lib")).as_deref(), Some("//lib:srcs"));
        assert_eq!(key("srcs", Some("lib")).as_deref(), Some("//lib:srcs"));
        assert_eq!(
            key("main.sh", Some("app/cli")).as_deref(),
            Some("//app/cli:main.sh")
        );
        assert_eq!(key(":beacon", Some("")).as_deref(), Some("//:beacon"));

        // No package means no answer. A file in no package has nothing for a
        // relative label to be relative to.
        assert_eq!(key(":srcs", None), None);
        assert_eq!(key("srcs", None), None);
    }

    #[test]
    fn the_main_repository_may_be_named_explicitly() {
        assert_eq!(key("@//lib:srcs", None).as_deref(), Some("//lib:srcs"));
        assert_eq!(key("@@//lib:srcs", None).as_deref(), Some("//lib:srcs"));
    }

    /// An apparent repo name maps to a canonical one that only `bazel mod
    /// dump_repo_mapping` knows, and the repo may not be fetched at all.
    /// Guessing `@rules_go` is `rules_go+` was already wrong once, when the
    /// format changed in Bazel 8.
    #[test]
    fn external_repositories_are_refused() {
        assert_eq!(key("@platforms//os:linux", Some("lib")), None);
        assert_eq!(
            key("@bazel_skylib//rules:write_file.bzl", Some("lib")),
            None
        );
        assert_eq!(key("@@rules_go+//go:def.bzl", Some("lib")), None);
        assert_eq!(key("@sh//:__subpackages__", Some("lib")), None);
    }

    /// A pattern names a set. Picking one of its members would be a guess, and
    /// invariant 4 rates a wrong jump worse than no jump.
    #[test]
    fn target_patterns_are_not_labels() {
        assert_eq!(key("//lib:all", None), None);
        assert_eq!(key("//lib:*", None), None);
        assert_eq!(key("//lib:all-targets", None), None);
        assert_eq!(key("//...", None), None);
        assert_eq!(key("//lib/...", None), None);
        assert_eq!(key("...", Some("lib")), None);
        assert_eq!(key(":all", Some("lib")), None);
    }

    #[test]
    fn strings_that_are_not_labels_resolve_to_nothing() {
        assert_eq!(key("", Some("lib")), None);
        assert_eq!(key(":", Some("lib")), None);
        assert_eq!(key("//", None), None);
        assert_eq!(key("//lib:", None), None);
        assert_eq!(key("///lib:srcs", None), None);
        // Prose, and a shell command with a label buried in it. Neither is one.
        assert_eq!(key("hello world", Some("lib")), None);
        assert_eq!(key("cat $(location :srcs) > $@", Some("lib")), None);
        // A relative label may not carry a package: `pkg:target` is not legal.
        assert_eq!(key("lib:srcs", Some("app")), None);
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
