//! Immutable workspace configuration graph.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use arc_swap::ArcSwap;
use rustc_hash::FxHashMap;

use super::commands;
use super::syntax::{
    Directive, Parse, Span, Statement, config_declaration, config_references, parse,
};

/// One rc file read from disk.
#[derive(Debug)]
pub struct ConfigurationFile {
    pub path: Arc<Path>,
    pub text: Arc<str>,
    pub parsed: Parse,
}

/// An ordinary option entry after imports have been expanded in place.
#[derive(Debug, Clone)]
pub struct Entry {
    pub file: Arc<ConfigurationFile>,
    pub line: usize,
}

/// A named configuration declaration or reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigSite {
    pub name: Box<str>,
    pub command: Box<str>,
    pub file: Arc<Path>,
    pub range: Span,
}

/// One import after workspace-relative resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportSite {
    pub file: Arc<Path>,
    pub range: Span,
    pub target: PathBuf,
    pub loaded: Option<Arc<Path>>,
    pub active: bool,
}

/// Severity of a graph-level import finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProblemSeverity {
    Error,
    Warning,
}

/// A problem found while following the import graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub file: Arc<Path>,
    pub range: Span,
    pub severity: ProblemSeverity,
    pub message: Box<str>,
}

/// The last-saved workspace rc graph.
#[derive(Debug, Default)]
pub struct ConfigurationSnapshot {
    pub root: Option<Arc<Path>>,
    pub files: FxHashMap<PathBuf, Arc<ConfigurationFile>>,
    pub entries: Vec<Entry>,
    pub declarations: Vec<ConfigSite>,
    pub references: Vec<ConfigSite>,
    pub imports: Vec<ImportSite>,
    pub problems: Vec<Problem>,
}

impl ConfigurationSnapshot {
    /// Build from the conventional workspace `.bazelrc`, if it exists.
    #[must_use]
    pub fn build(root: &Path) -> Self {
        let mut builder = Builder {
            root,
            snapshot: Self {
                root: Some(Arc::from(root)),
                ..Self::default()
            },
            active: Vec::new(),
        };
        drop(builder.visit(&root.join(".bazelrc"), None, false));
        builder.snapshot
    }

    pub fn declarations(&self, name: &str) -> impl Iterator<Item = &ConfigSite> {
        self.declarations
            .iter()
            .filter(move |site| site.name.as_ref() == name)
    }
}

/// Independently published configuration snapshots.
#[derive(Clone)]
pub struct ConfigurationHandle(Arc<ArcSwap<ConfigurationSnapshot>>);

impl ConfigurationHandle {
    #[must_use]
    pub fn new() -> Self {
        Self(Arc::new(ArcSwap::from_pointee(
            ConfigurationSnapshot::default(),
        )))
    }

    #[must_use]
    pub fn load(&self) -> Arc<ConfigurationSnapshot> {
        self.0.load_full()
    }

    pub fn store(&self, snapshot: ConfigurationSnapshot) {
        self.0.store(Arc::new(snapshot));
    }
}

impl Default for ConfigurationHandle {
    fn default() -> Self {
        Self::new()
    }
}

struct Builder<'a> {
    root: &'a Path,
    snapshot: ConfigurationSnapshot,
    active: Vec<PathBuf>,
}

impl Builder<'_> {
    fn visit(&mut self, path: &Path, origin: Option<Origin>, optional: bool) -> Option<Arc<Path>> {
        let canonical = match std::fs::canonicalize(path) {
            Ok(path) => path,
            Err(err) => {
                if !optional && let Some(origin) = origin {
                    self.problem(
                        &origin,
                        ProblemSeverity::Error,
                        format!("could not import {}: {err}", path.display()),
                    );
                }
                return None;
            }
        };
        if self.active.contains(&canonical) {
            if let Some(origin) = origin {
                self.problem(
                    &origin,
                    ProblemSeverity::Error,
                    format!("configuration import cycle through {}", canonical.display()),
                );
            }
            return Some(Arc::from(canonical));
        }

        let repeated = self.snapshot.files.contains_key(&canonical);
        let file = if let Some(file) = self.snapshot.files.get(&canonical) {
            Arc::clone(file)
        } else {
            let text = match std::fs::read_to_string(&canonical) {
                Ok(text) => text,
                Err(err) => {
                    if !optional && let Some(origin) = origin {
                        self.problem(
                            &origin,
                            ProblemSeverity::Error,
                            format!("could not import {}: {err}", canonical.display()),
                        );
                    }
                    return None;
                }
            };
            let file = Arc::new(ConfigurationFile {
                path: Arc::from(canonical.as_path()),
                parsed: parse(&text),
                text: Arc::from(text),
            });
            self.snapshot
                .files
                .insert(canonical.clone(), Arc::clone(&file));
            file
        };
        if repeated && let Some(origin) = &origin {
            self.problem(
                origin,
                ProblemSeverity::Warning,
                format!(
                    "configuration imports {} more than once",
                    canonical.display()
                ),
            );
        }

        self.active.push(canonical);
        for (line_number, line) in file.parsed.lines.iter().enumerate() {
            match &line.statement {
                Some(Statement::Entry) => self.entry(Arc::clone(&file), line_number),
                Some(Statement::Directive(directive)) => {
                    let active = !matches!(directive, Directive::ConditionalImport(condition) if !condition.matches("8.7.0"));
                    let path_token = if matches!(directive, Directive::ConditionalImport(_)) {
                        &line.tokens[2]
                    } else {
                        &line.tokens[1]
                    };
                    let target = resolve_import(self.root, &path_token.text);
                    let loaded = active.then(|| {
                        self.visit(
                            &target,
                            Some(Origin {
                                file: Arc::clone(&file.path),
                                range: path_token.range,
                            }),
                            !matches!(directive, Directive::Import),
                        )
                    });
                    self.snapshot.imports.push(ImportSite {
                        file: Arc::clone(&file.path),
                        range: path_token.range,
                        target,
                        loaded: loaded.flatten(),
                        active,
                    });
                }
                Some(Statement::InvalidDirective) | None => {}
            }
        }
        self.active.pop();
        Some(file.path.clone())
    }

    fn entry(&mut self, file: Arc<ConfigurationFile>, line_number: usize) {
        let line = &file.parsed.lines[line_number];
        let key = &line.tokens[0];
        if let Some((key, command, name)) = config_declaration(line)
            && commands::accepts_config(command)
        {
            self.snapshot.declarations.push(ConfigSite {
                name: name.into(),
                command: command.into(),
                file: Arc::clone(&file.path),
                range: key.range,
            });
        }

        let command = key
            .text
            .split_once(':')
            .map_or(key.text.as_str(), |(base, _)| base);
        if commands::accepts_config(command) {
            for reference in config_references(line, &file.text) {
                self.snapshot.references.push(ConfigSite {
                    name: reference.name.into(),
                    command: command.into(),
                    file: Arc::clone(&file.path),
                    range: reference.range,
                });
            }
        }
        self.snapshot.entries.push(Entry {
            file,
            line: line_number,
        });
    }

    fn problem(&mut self, origin: &Origin, severity: ProblemSeverity, message: String) {
        self.snapshot.problems.push(Problem {
            file: Arc::clone(&origin.file),
            range: origin.range,
            severity,
            message: message.into(),
        });
    }
}

struct Origin {
    file: Arc<Path>,
    range: Span,
}

pub(super) fn resolve_import(root: &Path, raw: &str) -> PathBuf {
    if let Some(relative) = raw.strip_prefix("%workspace%/") {
        root.join(relative)
    } else {
        let path = Path::new(raw);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            root.join(path)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    struct Workspace(PathBuf);

    static NEXT_WORKSPACE: AtomicU64 = AtomicU64::new(0);

    impl Workspace {
        fn new() -> Self {
            let unique = NEXT_WORKSPACE.fetch_add(1, Ordering::Relaxed);
            let root =
                std::env::temp_dir().join(format!("bls-bazelrc-{}-{unique}", std::process::id()));
            std::fs::create_dir_all(root.join("config")).unwrap();
            Self(root)
        }

        fn write(&self, relative: &str, text: &str) {
            std::fs::write(self.0.join(relative), text).unwrap();
        }
    }

    impl Drop for Workspace {
        fn drop(&mut self) {
            drop(std::fs::remove_dir_all(&self.0));
        }
    }

    #[test]
    fn imports_expand_in_place_and_relative_to_the_workspace() {
        let workspace = Workspace::new();
        workspace.write(
            ".bazelrc",
            "build --define=before=1\nimport config/child.bazelrc\nbuild --define=after=1\n",
        );
        workspace.write(
            "config/child.bazelrc",
            "build:dev --define=child=1\nbuild --config=dev\n",
        );
        let snapshot = ConfigurationSnapshot::build(&workspace.0);
        assert_eq!(snapshot.files.len(), 2);
        assert_eq!(snapshot.entries.len(), 4);
        let child = std::fs::canonicalize(workspace.0.join("config/child.bazelrc")).unwrap();
        assert_eq!(snapshot.entries[1].file.path.as_ref(), child.as_path());
        assert_eq!(snapshot.declarations("dev").count(), 1);
        assert_eq!(snapshot.references[0].name.as_ref(), "dev");
    }

    #[test]
    fn only_effective_config_sections_enter_the_graph() {
        let workspace = Workspace::new();
        workspace.write(
            ".bazelrc",
            "build:empty\nstartup:dev --host_jvm_args=-Xmx1g\nfuture:dev --x\n\
             build:dev --define=mode=dev\nbuild --config=dev\nstartup --config=dev\n",
        );
        let snapshot = ConfigurationSnapshot::build(&workspace.0);
        assert_eq!(snapshot.declarations.len(), 1);
        assert_eq!(snapshot.declarations[0].name.as_ref(), "dev");
        assert_eq!(snapshot.references.len(), 1);
        assert_eq!(snapshot.references[0].command.as_ref(), "build");
    }

    #[test]
    fn optional_imports_are_quiet_and_hard_imports_are_not() {
        let workspace = Workspace::new();
        workspace.write(
            ".bazelrc",
            "try-import absent\nimport missing\ntry-import-if-bazel-version <8.7.0 also-absent\n",
        );
        let snapshot = ConfigurationSnapshot::build(&workspace.0);
        assert_eq!(snapshot.problems.len(), 1);
        assert!(snapshot.problems[0].message.contains("missing"));
    }

    #[test]
    fn a_cycle_is_an_error_but_a_diamond_replays_entries() {
        let workspace = Workspace::new();
        workspace.write(
            ".bazelrc",
            "import config/left.bazelrc\nimport config/right.bazelrc\n",
        );
        workspace.write(
            "config/left.bazelrc",
            "import config/shared.bazelrc\nbuild --define=left=1\n",
        );
        workspace.write(
            "config/right.bazelrc",
            "import config/shared.bazelrc\nbuild --define=right=1\n",
        );
        workspace.write(
            "config/shared.bazelrc",
            "try-import config/left.bazelrc\nbuild:shared --define=shared=1\n",
        );
        let snapshot = ConfigurationSnapshot::build(&workspace.0);
        assert_eq!(snapshot.entries.len(), 5);
        assert!(
            snapshot
                .problems
                .iter()
                .any(|problem| problem.message.contains("cycle"))
        );
        assert!(
            snapshot
                .problems
                .iter()
                .any(|problem| problem.severity == ProblemSeverity::Warning)
        );
    }
}
