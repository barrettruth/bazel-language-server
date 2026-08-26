//! Request handling against in-memory state.
//!
//! Everything here is parse, walk and convert against a snapshot. No Bazel:
//! that is invariant 1, expressed as a module boundary. The only filesystem
//! call is a `stat` asking whether a label names a source file, which costs
//! one syscall and cannot block on anything.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::label::{Label, make_variable_labels, parse_label};
use anyhow::{Result, bail};
use lsp_types::{
    BaseSymbolInformation, Diagnostic, DiagnosticSeverity, DocumentHighlight,
    DocumentHighlightKind, DocumentSymbol, Hover, Location, LocationLink, MarkupContent,
    MarkupKind, Position, Range, SymbolKind, TextEdit, Uri, WorkspaceEdit, WorkspaceSymbol,
};
use starlark_cst::ast::{Arg, AstNode, Expr, File, LiteralExpr, LoadItem, LoadStmt, Stmt};
use starlark_cst::{Dialect, FileKind, SyntaxElement, SyntaxKind, SyntaxNode, classify, parse};

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
pub fn workspace_symbols(index: &crate::index::Index, query: &str) -> Vec<WorkspaceSymbol> {
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
    let mut uri = String::from("file://");
    for byte in path.as_os_str().as_encoded_bytes() {
        // RFC 3986 unreserved, plus the separators a path needs to keep. Bytes
        // outside that set are escaped, so a workspace under a directory with a
        // space in it produces a URI a client can parse. `uri_to_path` decodes
        // the same way on the way back in.
        match byte {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'.'
            | b'_'
            | b'~'
            | b'/'
            | b':'
            | b'@' => uri.push(*byte as char),
            other => {
                use std::fmt::Write as _;
                let _ = write!(uri, "%{other:02X}");
            }
        }
    }
    uri.parse().ok()
}

/// The dialect and kind of a file in this workspace.
///
/// Both fall out of the path, so a handler derives them rather than taking them
/// as arguments that a caller could pass inconsistently with each other.
fn classify_file(file: &Path, root: &Path) -> (Dialect, FileKind) {
    classify(file, Some(root)).unwrap_or((Dialect::Standard, FileKind::Bzl))
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
fn string_at(root: &SyntaxNode, offset: u32, kind: FileKind) -> Option<StringAt> {
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
        // Only a BUILD file declares targets. `MODULE.bazel` and `.bzl` files
        // are full of top-level calls carrying a `name` — `module(name = "x")`,
        // every `bazel_dep` — and such a string is neither a declaration nor a
        // label. Reading it as either resolves it against the enclosing package
        // and lands on whichever target happens to share the spelling, which
        // rename would then rewrite.
        let role = match (declares_a_target(&literal), kind) {
            (true, FileKind::Build) => StringRole::TargetName,
            (true, _) => return None,
            (false, _) => StringRole::Label,
        };
        (literal.string_value()?, literal.string_value_range()?, role)
    };

    let range = u32::from(range.start())..u32::from(range.end());
    // The content range excludes the quotes, so a cursor on one is not on the
    // label. Its far end counts: that is where the caret sits after typing.
    if !(range.start..=range.end).contains(&offset) {
        return None;
    }

    // A `cmd` is not a label, but `$(location :srcs)` inside it holds one, and
    // that is what the cursor is on. The index already reads these, so without
    // this the same label is findable by references and not by navigation.
    if let Some(inner) = expansion_at(&value, range.start, offset) {
        return Some(inner);
    }

    Some(StringAt { value, range, role })
}

/// The label of a make-variable expansion under the cursor, in file coordinates.
///
/// `content` is where the string's content begins, and `string_value` returns
/// the raw slice rather than an unescaped copy, so an offset within the value
/// is that offset within the file.
fn expansion_at(value: &str, content: u32, offset: u32) -> Option<StringAt> {
    make_variable_labels(value).find_map(|(label, at)| {
        let start = content + u32::try_from(at).ok()?;
        let end = start + u32::try_from(label.len()).ok()?;
        (start..=end).contains(&offset).then_some(StringAt {
            value: label,
            range: start..end,
            role: StringRole::Label,
        })
    })
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
fn target_site(index: &crate::index::Index, label: &Label) -> Option<Site> {
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
    file: &Path,
    root: &Path,
    index: &crate::index::Index,
    position: Position,
) -> Vec<LocationLink> {
    let lines = LineIndex::new(text);
    let Ok(offset) = u32::try_from(lines.offset(text, position)) else {
        return Vec::new();
    };
    let Some(found) = string_at(
        &parse(text, classify_file(file, root).0).syntax(),
        offset,
        classify_file(file, root).1,
    ) else {
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

/// What the string under the cursor names.
///
/// Three strings get a card. A label is resolved to the target it names, or to
/// the source file where no rule declares one. A target's own `name` gets the
/// same card plus how many references the index holds. A `load()` path is
/// resolved to the `.bzl` file it reads.
///
/// Everything else declines — a symbol inside a `load()`, a `name` in a file
/// that declares no targets, prose, a string that is no label at all — because
/// saying anything more would mean opening a second file and resolving a name
/// in it, and there is no symbol table here to do that with. Rule and
/// attribute documentation is the same answer: it is per-workspace, comes from
/// `--proto:rule_classes`, and waits on the graph tier. See `ROADMAP.md` G5.
///
/// The label is always shown resolved — `//lib:srcs` where the author wrote
/// `":srcs"` — which is most of what makes the card worth reading.
#[must_use]
pub fn hover(
    text: &str,
    file: &Path,
    root: &Path,
    index: &crate::index::Index,
    position: Position,
) -> Option<Hover> {
    let lines = LineIndex::new(text);
    let offset = u32::try_from(lines.offset(text, position)).ok()?;
    let found = string_at(
        &parse(text, classify_file(file, root).0).syntax(),
        offset,
        classify_file(file, root).1,
    )?;
    let package = enclosing_package(root, file);

    let markdown = match &found.role {
        // A load path names a file, so the index is not consulted, exactly as
        // it is not in `definition`.
        StringRole::LoadModule => {
            let label = parse_label(&found.value, package.as_deref())?;
            let site = file_site(root, &label)?;
            Some(card(
                &label.key(),
                &format!("Starlark file `{}`", relative(root, &site.path)),
            ))
        }
        StringRole::LoadSymbol(_) => None,
        StringRole::TargetName => {
            let label = Label::new(package.as_deref()?, &found.value);
            let declared = declared_card(index, root, &label)?;
            Some(format!("{declared}\n\n{}", tally(index, &label)))
        }
        StringRole::Label => {
            let label = parse_label(&found.value, package.as_deref())?;
            declared_card(index, root, &label)
                .or_else(|| {
                    let site = file_site(root, &label)?;
                    Some(card(
                        &label.key(),
                        &format!("source file `{}`", relative(root, &site.path)),
                    ))
                })
                .or_else(|| {
                    tracing::debug!(
                        label = label.key(),
                        "no such target in the static index and no source file at its path, so \
                         there is nothing true to say about it; legacy macros and external \
                         repositories need the graph tier"
                    );
                    None
                })
        }
    }?;

    Some(Hover {
        contents: MarkupContent {
            kind: MarkupKind::Markdown,
            value: markdown,
        }
        .into(),
        // The label alone, without its quotes, so the client underlines what
        // the card is about.
        range: Some(Range {
            start: lines.position(text, found.range.start as usize),
            end: lines.position(text, found.range.end as usize),
        }),
    })
}

/// A card: the resolved label, fenced, and one line saying what it is.
///
/// The fence carries no language, because its contents are a label rather than
/// code and a Starlark highlighter renders `//lib:srcs` as punctuation.
fn card(label: &str, detail: &str) -> String {
    format!("```\n{label}\n```\n{detail}")
}

/// The card for a target the index has seen declared.
fn declared_card(index: &crate::index::Index, root: &Path, label: &Label) -> Option<String> {
    let target = index.target(&label.key())?;
    let file = index.path(target.file)?;
    Some(card(
        &label.key(),
        &format!("`{}` declared in `{}`", target.rule, relative(root, file)),
    ))
}

/// How many references the index holds, said as what it is.
///
/// The count is a property of the static index and not of the target: labels
/// that legacy macros compute are invisible to it, so the true number is this
/// one or larger. A bare "5 references" would be read as the answer.
fn tally(index: &crate::index::Index, label: &Label) -> String {
    let counted = match index.references(&label.key()).len() {
        0 => "No references".to_string(),
        1 => "1 reference".to_string(),
        n => format!("{n} references"),
    };
    format!("{counted} in the static index, which does not see macro-generated labels")
}

/// A path as a label spells it: relative to the workspace, forward slashes.
///
/// A path from outside the workspace keeps its own spelling rather than being
/// dropped — an absolute path is still true, and a card with a hole in it is
/// not.
fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
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
    file: &Path,
    root: &Path,
    index: &crate::index::Index,
    position: Position,
    include_declaration: bool,
) -> Vec<Location> {
    let lines = LineIndex::new(text);
    let Ok(offset) = u32::try_from(lines.offset(text, position)) else {
        return Vec::new();
    };
    let Some(found) = string_at(
        &parse(text, classify_file(file, root).0).syntax(),
        offset,
        classify_file(file, root).1,
    ) else {
        return Vec::new();
    };

    let package = enclosing_package(root, file);
    let Some(label) = target_label(&found, package.as_deref()) else {
        return Vec::new();
    };

    let key = label.key();
    let sites = name_sites(index, &key, include_declaration);
    tracing::debug!(label = key, count = sites.len(), "references");
    sites
        .into_iter()
        .filter_map(|(path, range)| {
            Some(Location {
                uri: file_uri(&path)?,
                range,
            })
        })
        .collect()
}

/// Every occurrence of the target under the cursor, within one file.
///
/// `references` narrowed to the document the cursor is in, which is what an
/// editor paints as you rest on a label. The declaration is a `Write` and the
/// labels naming it are `Read`s, so a client can colour the definition apart
/// from its uses.
///
/// Partial in the same two ways `references` is: external repositories and the
/// targets legacy macros compute at evaluation time wait on the graph tier.
#[must_use]
pub fn document_highlight(
    text: &str,
    file: &Path,
    root: &Path,
    index: &crate::index::Index,
    position: Position,
) -> Vec<DocumentHighlight> {
    let lines = LineIndex::new(text);
    let label = u32::try_from(lines.offset(text, position))
        .ok()
        .and_then(|offset| {
            string_at(
                &parse(text, classify_file(file, root).0).syntax(),
                offset,
                classify_file(file, root).1,
            )
        })
        .and_then(|found| target_label(&found, enclosing_package(root, file).as_deref()));
    let Some(label) = label else {
        tracing::debug!("the cursor is on no label and no target name, so nothing is highlighted");
        return Vec::new();
    };

    let key = label.key();
    let declaration = declaration_site(index, &key);
    let highlights: Vec<DocumentHighlight> = name_sites(index, &key, true)
        .into_iter()
        .filter(|site| site.0 == file)
        .map(|site| DocumentHighlight {
            range: site.1,
            kind: Some(if declaration.as_ref() == Some(&site) {
                DocumentHighlightKind::Write
            } else {
                DocumentHighlightKind::Read
            }),
        })
        .collect();
    tracing::debug!(label = key, count = highlights.len(), "documentHighlight");
    highlights
}

/// The target a string names, whether it declares the target or points at it.
///
/// A declaration names its own package; a label has to be resolved against the
/// package that wrote it. A `.bzl` file is not a target at all: finding the
/// files that `load()` it is a different question, and answering this one with
/// those would be a wrong answer rather than none.
fn target_label(found: &StringAt, package: Option<&str>) -> Option<Label> {
    match &found.role {
        StringRole::TargetName => Some(Label::new(package?, &found.value)),
        StringRole::Label => parse_label(&found.value, package),
        StringRole::LoadModule | StringRole::LoadSymbol(_) => None,
    }
}

/// Every place a target's name is written, as `(file, range of the name)`.
///
/// The range covers the name alone — `srcs` inside `"//lib:srcs"` — which is
/// what a client highlights and what a rename replaces.
///
/// Sorted, so a picker does not reshuffle between calls and a file's edits
/// arrive in the order they appear in it.
fn name_sites(
    index: &crate::index::Index,
    key: &str,
    include_declaration: bool,
) -> Vec<(PathBuf, Range)> {
    let mut sites: Vec<(PathBuf, Range)> = index
        .references(key)
        .iter()
        .filter_map(|reference| {
            Some((
                index.path(reference.file)?.to_path_buf(),
                name_range(reference.line, reference.character, reference.length),
            ))
        })
        .collect();

    if include_declaration && let Some(site) = declaration_site(index, key) {
        sites.push(site);
    }

    sites.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(a.1.start.line.cmp(&b.1.start.line))
            .then(a.1.start.character.cmp(&b.1.start.character))
    });
    sites.dedup();
    sites
}

/// Where the target's own `name` is written, in the same shape as the sites
/// referring to it, so the declaration is recognisable among them.
fn declaration_site(index: &crate::index::Index, key: &str) -> Option<(PathBuf, Range)> {
    let target = index.target(key)?;
    Some((
        index.path(target.file)?.to_path_buf(),
        name_range(target.line, target.character, target.length),
    ))
}

/// A name's range, from the UTF-16 columns the index recorded. A name never
/// spans a line, so its end is its start plus its length.
fn name_range(line: u32, character: u32, length: u32) -> Range {
    Range {
        start: Position { line, character },
        end: Position {
            line,
            character: character + length,
        },
    }
}

/// The punctuation Bazel allows in a target name, alongside `a-zA-Z0-9`.
const NAME_PUNCTUATION: &str = "!%-@^_\"#$&'()*-+,;<=>?[]{|}~/.";

/// Refuse a name Bazel could not load.
///
/// A rename that writes an illegal name breaks every file that mentioned the
/// target, which the user discovers at their next build. An error the editor
/// shows is the one outcome they can act on.
fn validate_name(name: &str) -> Result<()> {
    let allowed = |c: char| c.is_ascii_alphanumeric() || NAME_PUNCTUATION.contains(c);
    if !name.is_empty()
        && !name.starts_with('/')
        && !name.ends_with('/')
        && name.chars().all(allowed)
    {
        return Ok(());
    }
    bail!(
        "{name:?} is not a Bazel target name: a name holds a-zA-Z0-9 and {NAME_PUNCTUATION}, \
         has no whitespace, has at least one character, and neither starts nor ends with /"
    )
}

/// Rename the target under the cursor, rewriting every label that names it.
///
/// The cursor may be on a label (`"//lib:srcs"`, `":srcs"`) or on the `name` of
/// the rule declaring it; both rename the same target. Only the name is
/// rewritten: `":srcs"` becomes `":sources"` and `"//lib:srcs"` becomes
/// `"//lib:sources"`, package and colon as the author wrote them.
///
/// **As complete as the index is, in two ways a caller must not paper over.**
/// External repositories are not searched, because resolving `@repo//…` needs
/// the repo mapping only Bazel can produce. And the static tier cannot see
/// targets or references that legacy macros compute at evaluation time — a
/// macro emitting `deps = [name + "_lib"]` is invisible here, so a label it
/// generates keeps the old name. Both wait on the graph tier; see
/// `ROADMAP.md` G4.
///
/// A new name Bazel could not load is an error rather than an empty result,
/// because an editor shows it and a broken workspace is the worse outcome.
///
/// # Errors
///
/// When `new_name` is not a legal Bazel target name.
pub fn rename(
    text: &str,
    file: &Path,
    root: &Path,
    index: &crate::index::Index,
    position: Position,
    new_name: &str,
) -> Result<Option<WorkspaceEdit>> {
    validate_name(new_name)?;

    let Some((_, key)) = renameable(text, file, root, index, position) else {
        return Ok(None);
    };

    let mut changes: HashMap<Uri, Vec<TextEdit>> = HashMap::new();
    for (path, range) in name_sites(index, &key, true) {
        let Some(uri) = file_uri(&path) else { continue };
        changes.entry(uri).or_default().push(TextEdit {
            range,
            new_text: new_name.to_string(),
        });
    }

    tracing::debug!(label = key, files = changes.len(), "rename");
    Ok(Some(WorkspaceEdit {
        changes: Some(changes),
        ..Default::default()
    }))
}

/// The range `textDocument/rename` would replace under the cursor, or nothing
/// where there is no target to rename.
///
/// It is the name alone — `srcs` out of `"//lib:srcs"` — so an editor seeds
/// its prompt with the name the user is changing. Declining tells the editor
/// not to offer a rename that would come back empty.
#[must_use]
pub fn prepare_rename(
    text: &str,
    file: &Path,
    root: &Path,
    index: &crate::index::Index,
    position: Position,
) -> Option<Range> {
    let (name, _) = renameable(text, file, root, index, position)?;
    let lines = LineIndex::new(text);
    Some(Range {
        start: lines.position(text, name.start as usize),
        end: lines.position(text, name.end as usize),
    })
}

/// The name under the cursor as a byte range, and the target it renames.
///
/// Only a declared target can be renamed. A label naming a source file, an
/// output file or nothing at all has no declaration to rewrite, and rewriting
/// the labels alone would point every one of them at a target that does not
/// exist — invariant 4.
fn renameable(
    text: &str,
    file: &Path,
    root: &Path,
    index: &crate::index::Index,
    position: Position,
) -> Option<(std::ops::Range<u32>, String)> {
    let lines = LineIndex::new(text);
    let offset = u32::try_from(lines.offset(text, position)).ok()?;
    let found = string_at(
        &parse(text, classify_file(file, root).0).syntax(),
        offset,
        classify_file(file, root).1,
    )?;
    let label = target_label(&found, enclosing_package(root, file).as_deref())?;

    let key = label.key();
    if index.target(&key).is_none() {
        tracing::debug!(
            label = key,
            "no such target in the static index, so there is no declaration to rename; \
             legacy macros and external repositories need the graph tier"
        );
        return None;
    }

    let name_offset = u32::try_from(label.name_offset(&found.value)).ok()?;
    Some((found.range.start + name_offset..found.range.end, key))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUILD: &str = "\
filegroup(\n    name = \"srcs\",\n    srcs = [],\n)\n\ncc_library(name = \"core\")\n";

    /// A path a client can parse, for every path a filesystem allows.
    ///
    /// `path.display()` is raw, so a space produced `file:///ws with space/…`,
    /// which fluent-uri rejects; the request then failed and, before the loop
    /// caught its own errors, took the server with it.
    #[test]
    fn file_uri_escapes_what_a_uri_cannot_hold() {
        let cases = [
            ("/ws/lib/BUILD.bazel", "file:///ws/lib/BUILD.bazel"),
            ("/ws with space/BUILD", "file:///ws%20with%20space/BUILD"),
            ("/ws/a#b?c/BUILD", "file:///ws/a%23b%3Fc/BUILD"),
            ("/ws/100%/BUILD", "file:///ws/100%25/BUILD"),
        ];
        for (path, expected) in cases {
            let uri = file_uri(Path::new(path)).expect("a parseable uri");
            assert_eq!(uri.as_str(), expected, "encoding {path}");
        }
    }

    /// `name` identifies a declaration only at the top level. Nested, it is an
    /// argument that happens to share the spelling, and counting it as a
    /// declaration would hang every reference off the wrong label.
    #[test]
    fn only_a_top_level_name_declares_a_target() {
        let role = |text: &str, needle: &str| {
            let offset = u32::try_from(text.find(needle).expect("needle") + 1).unwrap();
            string_at(
                &parse(text, Dialect::Bazel).syntax(),
                offset,
                FileKind::Build,
            )
            .map(|found| found.role)
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
        let fixture = Fixture::workspace();
        let file = fixture.root.join("lib/BUILD.bazel");
        let text = std::fs::read_to_string(&file).expect("fixture");
        let offset = text.find("\"srcs\"").expect("the srcs declaration") + 2;
        let lines = LineIndex::new(&text);

        let jumps = definition(
            &text,
            &file,
            &fixture.root,
            &crate::index::Index::default(),
            lines.position(&text, offset),
        );
        assert!(jumps.is_empty(), "got {jumps:?}");
    }

    /// From the declaration and from a label pointing at it, the answer is the
    /// same target — and `includeDeclaration` is what decides whether the
    /// declaring line is in it.
    #[test]
    fn references_agree_from_either_end() {
        let fixture = Fixture::workspace();
        let index = crate::index::build_static(&fixture.root);
        let file = fixture.root.join("lib/BUILD.bazel");
        let text = std::fs::read_to_string(&file).expect("fixture");
        let lines = LineIndex::new(&text);

        let at = |needle: &str, skip: usize| {
            let offset = text.find(needle).expect("needle") + skip;
            references(
                &text,
                &file,
                &fixture.root,
                &index,
                lines.position(&text, offset),
                false,
            )
        };

        // `//lib:srcs` is referenced from the alias, the genrule's srcs, a
        // select() branch, a rule attribute, and inside the genrule cmd's
        // `$(location :srcs)`, which is a label a whole-string parse cannot see.
        let from_declaration = at("\"srcs\"", 2);
        assert_eq!(
            from_declaration.len(),
            5,
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
    fn a_load_path_names_no_target() {
        let fixture = Fixture::workspace();
        let index = crate::index::build_static(&fixture.root);
        let file = fixture.root.join("lib/BUILD.bazel");
        let text = std::fs::read_to_string(&file).expect("fixture");
        let lines = LineIndex::new(&text);
        let offset = text.find("//macros:legacy.bzl").expect("a load path") + 4;

        assert!(
            references(
                &text,
                &file,
                &fixture.root,
                &index,
                lines.position(&text, offset),
                true,
            )
            .is_empty()
        );
        assert!(
            fixture
                .highlights("lib/BUILD.bazel", "//macros:legacy.bzl")
                .is_empty()
        );
    }

    /// The declaration is the write and every label naming it is a read, so an
    /// editor can colour the definition apart from its uses. Both ends of the
    /// question agree, as they do for references.
    #[test]
    fn document_highlight_writes_the_declaration_and_reads_its_labels() {
        let fixture = Fixture::workspace();
        let expected = [
            "Write 11:12 srcs",
            "Read 35:15 srcs",
            "Read 58:10 srcs",
            "Read 63:27 srcs",
            "Read 78:24 srcs",
            "Read 88:12 srcs",
        ];

        let from_declaration = fixture.highlights("lib/BUILD.bazel", "\"srcs\"");
        assert_eq!(from_declaration, expected);
        // From a label pointing at it: `actual = ":srcs"`.
        assert_eq!(
            fixture.highlights("lib/BUILD.bazel", "\":srcs\","),
            expected
        );
    }

    /// Only this document. `//lib/sub:sub_srcs` is named three times in
    /// `//lib` and declared in `//lib/sub`, and neither file sees the other's
    /// occurrences — a highlight in a buffer the user is not looking at is a
    /// range the client would paint over the wrong text.
    #[test]
    fn document_highlight_stops_at_the_file_it_was_asked_about() {
        let fixture = Fixture::workspace();
        assert_eq!(
            fixture.highlights("lib/BUILD.bazel", "//lib/sub:sub_srcs"),
            [
                "Read 23:23 sub_srcs",
                "Read 59:19 sub_srcs",
                "Read 63:55 sub_srcs"
            ]
        );
        // The declaring file holds the write and none of the reads.
        assert_eq!(
            fixture.highlights("lib/sub/BUILD.bazel", "\"sub_srcs\""),
            ["Write 3:12 sub_srcs"]
        );
    }

    /// A cursor on an identifier, a comment or bare punctuation is on no
    /// target, and the empty answer is logged rather than silent.
    #[test]
    fn document_highlight_declines_off_a_string() {
        let fixture = Fixture::workspace();
        for needle in ["filegroup(", "# Cross-package", "cc_library_placeholder"] {
            let found = fixture.highlights("lib/BUILD.bazel", needle);
            assert!(found.is_empty(), "cursor on {needle:?} found {found:?}");
        }
    }

    const LIB: &str = r#"filegroup(
    name = "srcs",
    srcs = ["a.txt"],
)

alias(
    name = "aliased",
    actual = ":srcs",
)
"#;

    const APP: &str = r#"filegroup(
    name = "app_srcs",
    srcs = ["//lib:srcs"],
)
"#;

    /// A workspace on disk, so a rename can be indexed, applied, and the
    /// result compared to text in full.
    struct Renaming {
        root: PathBuf,
        index: crate::index::Index,
    }

    impl Renaming {
        /// `//lib:srcs`: declared in `lib`, named relatively from `lib` and
        /// absolutely from `app`.
        fn workspace(name: &str) -> Self {
            let root = std::env::temp_dir().join(name);
            std::fs::remove_dir_all(&root).ok();
            for (relative, text) in [("lib/BUILD.bazel", LIB), ("app/BUILD.bazel", APP)] {
                let path = root.join(relative);
                std::fs::create_dir_all(path.parent().expect("a package directory")).unwrap();
                std::fs::write(path, text).unwrap();
            }
            let index = crate::index::build_static(&root);
            Self { root, index }
        }

        /// The document, and the cursor in the middle of `needle`.
        fn cursor(&self, relative: &str, needle: &str) -> (PathBuf, String, Position) {
            let file = self.root.join(relative);
            let text = std::fs::read_to_string(&file).expect("fixture file");
            let at = text
                .find(needle)
                .unwrap_or_else(|| panic!("{needle:?} is not in {relative}"))
                + needle.len() / 2;
            let position = LineIndex::new(&text).position(&text, at);
            (file, text, position)
        }

        fn rename(
            &self,
            relative: &str,
            needle: &str,
            new_name: &str,
        ) -> Result<Option<WorkspaceEdit>> {
            let (file, text, position) = self.cursor(relative, needle);
            rename(&text, &file, &self.root, &self.index, position, new_name)
        }

        /// The text `prepareRename` would put in the editor's prompt.
        fn prepared(&self, relative: &str, needle: &str) -> Option<String> {
            let (file, text, position) = self.cursor(relative, needle);
            let range = prepare_rename(&text, &file, &self.root, &self.index, position)?;
            let lines = LineIndex::new(&text);
            Some(text[lines.offset(&text, range.start)..lines.offset(&text, range.end)].to_string())
        }

        /// One file, as an editor applying the edits would leave it.
        fn applied(&self, edit: &WorkspaceEdit, relative: &str) -> String {
            let path = self.root.join(relative);
            let text = std::fs::read_to_string(&path).expect("fixture file");
            let uri = file_uri(&path).expect("a uri");
            let mut edits = edit
                .changes
                .as_ref()
                .and_then(|changes| changes.get(&uri))
                .cloned()
                .unwrap_or_default();
            edits.sort_by_key(|edit| (edit.range.start.line, edit.range.start.character));

            let lines = LineIndex::new(&text);
            let mut applied = text.clone();
            // Back to front, so an applied edit does not move the next one.
            for edit in edits.iter().rev() {
                let start = lines.offset(&text, edit.range.start);
                let end = lines.offset(&text, edit.range.end);
                applied.replace_range(start..end, &edit.new_text);
            }
            applied
        }
    }

    impl Drop for Renaming {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.root).ok();
        }
    }

    /// From the declaration and from a label pointing at it, the answer is the
    /// same target and so are the edits.
    #[test]
    fn rename_agrees_from_either_end() {
        let workspace = Renaming::workspace("bls-rename-either-end");
        let from_declaration = workspace
            .rename("lib/BUILD.bazel", "\"srcs\"", "sources")
            .expect("a legal name");
        assert!(from_declaration.is_some());

        for (file, needle) in [
            ("lib/BUILD.bazel", "\":srcs\""),
            ("app/BUILD.bazel", "\"//lib:srcs\""),
        ] {
            assert_eq!(
                workspace
                    .rename(file, needle, "sources")
                    .expect("a legal name"),
                from_declaration,
                "renaming from {needle} in {file}"
            );
        }
    }

    /// Applied, the edits change the name and nothing else: `//lib:` keeps its
    /// package, `:srcs` keeps its colon, and the declaration moves with them.
    #[test]
    fn rename_rewrites_the_name_within_every_label() {
        let workspace = Renaming::workspace("bls-rename-applied");
        let edit = workspace
            .rename("lib/BUILD.bazel", "\"srcs\"", "sources")
            .expect("a legal name")
            .expect("a declared target");

        assert_eq!(
            workspace.applied(&edit, "lib/BUILD.bazel"),
            LIB.replace("name = \"srcs\"", "name = \"sources\"")
                .replace("actual = \":srcs\"", "actual = \":sources\"")
        );
        assert_eq!(
            workspace.applied(&edit, "app/BUILD.bazel"),
            APP.replace("\"//lib:srcs\"", "\"//lib:sources\"")
        );
    }

    /// Nothing refers to `//lib:aliased`, and it still renames: the
    /// declaration is a site like any other.
    #[test]
    fn a_target_with_no_references_renames_its_declaration() {
        let workspace = Renaming::workspace("bls-rename-unreferenced");
        let edit = workspace
            .rename("lib/BUILD.bazel", "\"aliased\"", "alias_target")
            .expect("a legal name")
            .expect("a declared target");

        let changes = edit.changes.as_ref().expect("changes");
        assert_eq!(changes.len(), 1, "one file, got {changes:?}");
        assert_eq!(
            workspace.applied(&edit, "lib/BUILD.bazel"),
            LIB.replace("\"aliased\"", "\"alias_target\"")
        );
    }

    /// A name Bazel could not load is refused rather than written: the user
    /// sees the error, instead of a workspace that stopped building.
    #[test]
    fn an_illegal_new_name_is_refused() {
        let workspace = Renaming::workspace("bls-rename-illegal-name");
        for illegal in [
            "",
            "two words",
            "tab\there",
            "/leading",
            "trailing/",
            "back\\slash",
        ] {
            assert!(
                workspace
                    .rename("lib/BUILD.bazel", "\"srcs\"", illegal)
                    .is_err(),
                "accepted {illegal:?}"
            );
        }
        // Bazel's alphabet is wider than an identifier's, and refusing a name
        // Bazel would take is its own kind of wrong.
        for legal in ["sources", "sub/dir.txt", "a+b", "v1.2.3-rc1"] {
            assert!(
                workspace
                    .rename("lib/BUILD.bazel", "\"srcs\"", legal)
                    .is_ok(),
                "refused {legal:?}"
            );
        }
    }

    /// Only a declared target can be renamed. `a.txt` is a source file: it has
    /// no declaration to rewrite, and rewriting the labels alone would point
    /// them all at nothing.
    #[test]
    fn a_label_naming_no_declaration_renames_nothing() {
        let workspace = Renaming::workspace("bls-rename-undeclared");
        assert_eq!(
            workspace
                .rename("lib/BUILD.bazel", "\"a.txt\"", "b.txt")
                .expect("a legal name"),
            None
        );
        assert_eq!(workspace.prepared("lib/BUILD.bazel", "\"a.txt\""), None);
    }

    /// The prompt an editor opens is seeded with the name, not the label that
    /// carries it.
    #[test]
    fn prepare_rename_selects_the_name_alone() {
        let workspace = Renaming::workspace("bls-prepare-rename");
        assert_eq!(
            workspace.prepared("app/BUILD.bazel", "\"//lib:srcs\""),
            Some("srcs".to_string())
        );
        assert_eq!(
            workspace.prepared("lib/BUILD.bazel", "\":srcs\""),
            Some("srcs".to_string())
        );
        assert_eq!(
            workspace.prepared("lib/BUILD.bazel", "\"srcs\""),
            Some("srcs".to_string())
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

    fn fixture_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/workspace")
            .canonicalize()
            .expect("the test workspace is checked in")
    }

    #[test]
    fn a_package_is_the_nearest_build_file() {
        let root = fixture_root();
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

    /// The index reads labels out of `$(location …)`, so navigation has to as
    /// well: a label that find-references reports and go-to-definition shrugs at
    /// looks like the definition is missing rather than the reader.
    #[test]
    fn a_label_inside_a_command_is_navigable() {
        let fixture = Fixture::workspace();
        let file = fixture.root.join("lib/BUILD.bazel");
        let text = std::fs::read_to_string(&file).expect("fixture");
        let lines = LineIndex::new(&text);
        let cmd = text.find("$(location :srcs)").expect("the genrule cmd");

        let on = |offset: usize| {
            definition(
                &text,
                &file,
                &fixture.root,
                &fixture.index,
                lines.position(&text, offset),
            )
        };

        // Inside the label, navigation resolves it.
        let jumps = on(cmd + "$(location :".len() + 1);
        assert_eq!(jumps.len(), 1, "got {jumps:?}");
        let origin = jumps[0].origin_selection_range.expect("an origin range");
        assert_eq!(
            &text[lines.offset(&text, origin.start)..lines.offset(&text, origin.end)],
            ":srcs",
            "the origin covers the label alone, not the command around it"
        );

        // On the prose around it, there is no label and nothing to offer.
        assert!(on(cmd.saturating_sub(2)).is_empty());
    }

    /// `module(name = "x")` is a top-level call with a `name`, and so is every
    /// `bazel_dep`. Reading one as a target declaration attributes the cursor
    /// to whichever BUILD target happens to share the spelling — and rename
    /// then rewrites that target and every label pointing at it, which is a
    /// corrupted workspace rather than a missing answer.
    ///
    /// Every handler that resolves a target name has to decline here, so this
    /// covers all of them rather than the one that happened to find it.
    #[test]
    fn only_a_build_file_declares_targets() {
        let root = PathBuf::from("/ws");
        let file = root.join("MODULE.bazel");
        let text = "module(name = \"beacon\")\n";
        let position = Position {
            line: 0,
            character: 15,
        };
        let index = crate::index::Index::default();

        // The same cursor in a BUILD file is a declaration, so the position is
        // right and the file kind is what decides.
        assert!(matches!(
            string_at(&parse(text, Dialect::Bazel).syntax(), 15, FileKind::Build)
                .map(|found| found.role),
            Some(StringRole::TargetName)
        ));
        assert!(
            string_at(&parse(text, Dialect::Bazel).syntax(), 15, FileKind::Module).is_none(),
            "a module's name is neither a declaration nor a label"
        );

        // `/ws/MODULE.bazel` classifies as a module, so every handler that
        // resolves a target name declines on it.
        assert!(references(text, &file, &root, &index, position, true).is_empty());
        assert!(document_highlight(text, &file, &root, &index, position).is_empty());
        assert!(prepare_rename(text, &file, &root, &index, position).is_none());
        assert!(
            rename(text, &file, &root, &index, position, "renamed")
                .expect("a legal name")
                .is_none()
        );
    }

    /// Drives a handler the way the server does: read the file, put the cursor
    /// in the middle of `needle`, and report where it lands.
    struct Fixture {
        root: PathBuf,
        index: crate::index::Index,
    }

    impl Fixture {
        fn workspace() -> Self {
            let root = fixture_root();
            let index = crate::index::build_static(&root);
            Self { root, index }
        }

        /// The document, and the cursor in the middle of `needle`.
        fn cursor(&self, relative: &str, needle: &str) -> (PathBuf, String, Position) {
            let file = self.root.join(relative);
            let text = std::fs::read_to_string(&file).expect("fixture file");
            let at = text.find(needle).unwrap_or_else(|| {
                panic!("{needle:?} is not in {relative}");
            }) + needle.len() / 2;
            let position = LineIndex::new(&text).position(&text, at);
            (file, text, position)
        }

        fn links(&self, relative: &str, needle: &str) -> Vec<LocationLink> {
            let (file, text, position) = self.cursor(relative, needle);
            definition(&text, &file, &self.root, &self.index, position)
        }

        /// Every highlight, as `kind line:character text`, so a test reads the
        /// range's own contents rather than taking its word for them.
        fn highlights(&self, relative: &str, needle: &str) -> Vec<String> {
            let (file, text, position) = self.cursor(relative, needle);
            let lines = LineIndex::new(&text);
            document_highlight(&text, &file, &self.root, &self.index, position)
                .into_iter()
                .map(|highlight| {
                    let start = lines.offset(&text, highlight.range.start);
                    let end = lines.offset(&text, highlight.range.end);
                    format!(
                        "{:?} {}:{} {}",
                        highlight.kind.expect("a kind"),
                        highlight.range.start.line,
                        highlight.range.start.character,
                        &text[start..end]
                    )
                })
                .collect()
        }

        /// The hover card, as the client would render it.
        fn card(&self, relative: &str, needle: &str) -> Option<String> {
            let (file, text, position) = self.cursor(relative, needle);
            let hovered = hover(&text, &file, &self.root, &self.index, position)?;
            match hovered.contents {
                lsp_types::Contents::MarkupContent(markup) => {
                    assert_eq!(markup.kind, MarkupKind::Markdown, "markdown, not marked-up");
                    Some(markup.value)
                }
                other => panic!("a card is markup content, got {other:?}"),
            }
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
        let fixture = Fixture::workspace();

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
        let fixture = Fixture::workspace();
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
        let fixture = Fixture::workspace();
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
        let fixture = Fixture::workspace();
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
        let fixture = Fixture::workspace();
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
        let root = fixture_root();
        let index = crate::index::Index::default();

        for needle in ["filegroup", "name", "srcs = [", ")"] {
            let at = text.find(needle).unwrap();
            let found = definition(
                text,
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
        let fixture = Fixture::workspace();
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

    /// A label's card says what the label resolves to: the target, the rule
    /// that declares it, and the file that holds it.
    #[test]
    fn hover_on_a_label_says_what_it_resolves_to() {
        let fixture = Fixture::workspace();

        assert_eq!(
            fixture
                .card("lib/BUILD.bazel", "//lib/sub:sub_srcs")
                .as_deref(),
            Some(
                "```\n//lib/sub:sub_srcs\n```\n\
                 `filegroup` declared in `lib/sub/BUILD.bazel`"
            )
        );
        // Written `":srcs"`, shown resolved: the package the file is in is
        // what the reader does not have to work out.
        assert_eq!(
            fixture.card("lib/BUILD.bazel", "\":srcs\",").as_deref(),
            Some("```\n//lib:srcs\n```\n`filegroup` declared in `lib/BUILD.bazel`")
        );
        // A label naming a source file rather than a declared target says so,
        // rather than claiming a rule it does not have.
        assert_eq!(
            fixture
                .card("lib/sub/BUILD.bazel", "//lib:exported.txt")
                .as_deref(),
            Some("```\n//lib:exported.txt\n```\nsource file `lib/exported.txt`")
        );
    }

    /// On the declaration, the card carries the reference count — worded so
    /// the number is read as what the index holds and not as the truth about
    /// the target, which only the graph tier can give.
    #[test]
    fn hover_on_a_declaration_counts_what_the_index_holds() {
        let fixture = Fixture::workspace();
        assert_eq!(
            fixture.card("lib/BUILD.bazel", "\"srcs\"").as_deref(),
            Some(
                "```\n//lib:srcs\n```\n\
                 `filegroup` declared in `lib/BUILD.bazel`\n\n\
                 5 references in the static index, which does not see macro-generated labels"
            )
        );
        // Nothing names `//lib:double_aliased`, and none is not a failure.
        assert_eq!(
            fixture
                .card("lib/BUILD.bazel", "\"double_aliased\"")
                .as_deref(),
            Some(
                "```\n//lib:double_aliased\n```\n\
                 `alias` declared in `lib/BUILD.bazel`\n\n\
                 No references in the static index, which does not see macro-generated labels"
            )
        );
    }

    /// A `load()` path resolves to the file it reads, and says nothing about
    /// what is in it: that would need a second parse and a symbol table.
    #[test]
    fn hover_on_a_load_path_resolves_the_file() {
        let fixture = Fixture::workspace();
        assert_eq!(
            fixture
                .card("lib/BUILD.bazel", "//macros:legacy.bzl")
                .as_deref(),
            Some("```\n//macros:legacy.bzl\n```\nStarlark file `macros/legacy.bzl`")
        );
        // Relative to the package doing the loading.
        assert_eq!(
            fixture.card("lib/BUILD.bazel", ":local.bzl").as_deref(),
            Some("```\n//lib:local.bzl\n```\nStarlark file `lib/local.bzl`")
        );
    }

    /// Everything the index cannot answer for declines. A card reading
    /// "unknown" is a claim about the target, when the only true claim is
    /// about the index.
    #[test]
    fn hover_declines_wherever_it_would_have_to_guess() {
        let fixture = Fixture::workspace();
        let nothing = [
            // The torture workspace's deliberately dangling label.
            ("lib/sub/BUILD.bazel", "//lib:does_not_exist"),
            // An external repository: the canonical name is Bazel's to know.
            ("lib/BUILD.bazel", "@platforms//os:linux"),
            // A pseudo-label that never names a target.
            ("lib/BUILD.bazel", "//visibility:public"),
            // A symbol inside a `load()`. The file it comes from is known and
            // what the symbol *is* is not, and the second is what was asked.
            ("lib/BUILD.bazel", "\"local_helper\""),
            // A cursor on an identifier, a comment, or a rule name.
            ("lib/BUILD.bazel", "filegroup("),
            ("lib/BUILD.bazel", "# Cross-package"),
            ("lib/BUILD.bazel", "cc_library_placeholder"),
        ];
        for (file, needle) in nothing {
            let found = fixture.card(file, needle);
            assert_eq!(found, None, "cursor on {needle:?} in {file}");
        }
    }

    /// Only a BUILD file declares targets. `module(name = "beacon")` names a
    /// module, and a repository with a `//:beacon` alias in it would otherwise
    /// get a card describing that alias — an answer about something the cursor
    /// is not on, which invariant 4 rates worse than no answer.
    #[test]
    fn a_name_outside_a_build_file_declares_no_target() {
        let root = std::env::temp_dir().join("bls-hover-module-name");
        std::fs::remove_dir_all(&root).ok();
        std::fs::create_dir_all(&root).unwrap();
        let module = "module(name = \"beacon\")\n";
        std::fs::write(root.join("MODULE.bazel"), module).unwrap();
        std::fs::write(
            root.join("BUILD.bazel"),
            "alias(name = \"beacon\", actual = \"//lib:srcs\")\n",
        )
        .unwrap();
        // The same text under two names: what decides is the kind of file, and
        // the kind comes from the path, so the test has to use real ones.
        std::fs::create_dir_all(root.join("pkg")).unwrap();
        std::fs::write(root.join("pkg/BUILD.bazel"), module).unwrap();
        let index = crate::index::build_static(&root);
        let lines = LineIndex::new(module);
        let at = module.find("beacon").expect("the module's name");
        let card = |relative: &str| {
            hover(
                module,
                &root.join(relative),
                &root,
                &index,
                lines.position(module, at),
            )
        };

        assert!(card("MODULE.bazel").is_none(), "a module is not a target");
        assert!(card("pkg/BUILD.bazel").is_some());

        std::fs::remove_dir_all(&root).ok();
    }

    /// The range is what the client highlights while the card is up. It has to
    /// be the label alone: including the quotes paints punctuation the user
    /// did not point at.
    #[test]
    fn the_hover_range_is_the_label_without_its_quotes() {
        let fixture = Fixture::workspace();
        let (file, text, position) = fixture.cursor("lib/BUILD.bazel", "//lib/sub:sub_srcs");
        let hovered = hover(&text, &file, &fixture.root, &fixture.index, position)
            .expect("the label resolves");

        let lines = LineIndex::new(&text);
        let range = hovered.range.expect("a range");
        let start = lines.offset(&text, range.start);
        let end = lines.offset(&text, range.end);
        assert_eq!(&text[start..end], "//lib/sub:sub_srcs");
    }
}
