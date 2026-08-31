//! Conservative diagnostics derived from Bazel 8.7 syntax and exact catalogs.

use lsp_types::{Diagnostic, DiagnosticSeverity, Range};

use super::catalog::is_build_setting;
use super::syntax::{Line, Span, Statement, config_references};
use super::{
    ConfigurationSnapshot, ConfigurationView, Flag, FlagCatalog, FlagSpelling, ProblemSeverity,
    commands, native_options,
};
use crate::document::{Document, Documents};

const SOURCE: &str = "bazel-language-server";

/// Findings in one current Bazelrc buffer.
#[must_use]
pub fn diagnostics(
    document: &Document,
    documents: &Documents,
    configuration: &ConfigurationSnapshot,
    catalog: Option<&FlagCatalog>,
) -> Vec<Diagnostic> {
    let Some(parsed) = document.bazelrc() else {
        return Vec::new();
    };
    let mut found: Vec<_> = parsed
        .errors
        .iter()
        .map(|error| {
            finding(
                document,
                error.range,
                DiagnosticSeverity::Error,
                &error.message,
            )
        })
        .collect();
    let saved_is_current = configuration
        .files
        .get(document.path())
        .is_some_and(|file| file.text.as_ref() == document.text());
    if saved_is_current {
        found.extend(
            configuration
                .problems
                .iter()
                .filter(|problem| problem.file.as_ref() == document.path())
                .map(|problem| {
                    finding(
                        document,
                        problem.range,
                        match problem.severity {
                            ProblemSeverity::Error => DiagnosticSeverity::Error,
                            ProblemSeverity::Warning => DiagnosticSeverity::Warning,
                        },
                        &problem.message,
                    )
                }),
        );
    }
    let configuration = ConfigurationView::new(documents, configuration);

    for line in parsed
        .lines
        .iter()
        .filter(|line| matches!(line.statement, Some(Statement::Entry)))
    {
        diagnose_entry(document, &configuration, catalog, line, &mut found);
    }
    found
}

fn diagnose_entry(
    document: &Document,
    configuration: &ConfigurationView,
    catalog: Option<&FlagCatalog>,
    line: &Line,
    found: &mut Vec<Diagnostic>,
) {
    let Some(key) = line.key() else { return };
    let command = key
        .text
        .split_once(':')
        .map_or(key.text.as_str(), |(command, _)| command);
    if !commands::NAMES.contains(&command) {
        found.push(finding(
            document,
            key.range,
            DiagnosticSeverity::Warning,
            &format!("`{command}` is not a Bazel 8.7 command or rc scope"),
        ));
        return;
    }

    diagnose_config_references(document, configuration, line, command, found);
    if key.text.contains(':') {
        for option in line
            .options()
            .iter()
            .filter(|option| option.text == "--config")
        {
            found.push(finding(
                document,
                option.range,
                DiagnosticSeverity::Error,
                "named configuration bodies require `--config=name` in Bazel 8.7",
            ));
        }
    }

    let Some(catalog) = catalog else { return };
    for option in native_options::uses(line, catalog) {
        if !option.option.text.starts_with('-') || is_build_setting(&option.option.text) {
            continue;
        }
        let Some(resolved) = option.resolved else {
            found.push(finding(
                document,
                option.option.range,
                DiagnosticSeverity::Warning,
                &format!(
                    "`{}` is not recognized by the Bazel 8.7 native flag catalog",
                    option
                        .option
                        .text
                        .split_once('=')
                        .map_or(option.option.text.as_str(), |(name, _)| name)
                ),
            ));
            continue;
        };
        let flag = resolved.flag;
        if !catalog.supports_scope(flag, command) {
            found.push(finding(
                document,
                option.option.range,
                DiagnosticSeverity::Error,
                &format!(
                    "`--{}` is not accepted in `{command}` rc entries by the Bazel 8.7 catalog",
                    flag.name
                ),
            ));
        }
        if matches!(
            resolved.spelling,
            FlagSpelling::Negative | FlagSpelling::NegativeOldName
        ) && option.option.text.contains('=')
        {
            found.push(finding(
                document,
                option.option.range,
                DiagnosticSeverity::Error,
                "a negative boolean flag cannot have a value",
            ));
        }
        if flag.requires_value && !option.option.text.contains('=') && option.value.is_none() {
            found.push(finding(
                document,
                option.option.range,
                DiagnosticSeverity::Error,
                &format!("`--{}` requires a value", flag.name),
            ));
        }
        diagnose_catalog_metadata(
            document,
            option.option.range,
            flag,
            resolved.spelling,
            found,
        );
    }
}

fn diagnose_config_references(
    document: &Document,
    configuration: &ConfigurationView,
    line: &Line,
    command: &str,
    found: &mut Vec<Diagnostic>,
) {
    if !configuration.ready() || !commands::accepts_config(command) {
        return;
    }
    for reference in config_references(line) {
        if configuration
            .applicable_declarations(command, &reference.name)
            .next()
            .is_none()
        {
            found.push(finding(
                document,
                reference.range,
                DiagnosticSeverity::Warning,
                &format!(
                    "configuration `{}` is not declared in the workspace Bazelrc graph",
                    reference.name
                ),
            ));
        }
    }
}

fn diagnose_catalog_metadata(
    document: &Document,
    range: Span,
    flag: &Flag,
    spelling: FlagSpelling,
    found: &mut Vec<Diagnostic>,
) {
    if matches!(
        spelling,
        FlagSpelling::OldName | FlagSpelling::NegativeOldName
    ) {
        let replacement = if spelling == FlagSpelling::NegativeOldName {
            format!("--no{}", flag.name)
        } else {
            format!("--{}", flag.name)
        };
        found.push(finding(
            document,
            range,
            DiagnosticSeverity::Warning,
            &format!("this is an old spelling; use `{replacement}`"),
        ));
    }

    if let Some(message) = flag
        .deprecation_warning
        .as_deref()
        .filter(|message| !message.is_empty())
    {
        found.push(finding(
            document,
            range,
            DiagnosticSeverity::Warning,
            message,
        ));
    } else if tagged(&flag.metadata_tags, "DEPRECATED") {
        found.push(finding(
            document,
            range,
            DiagnosticSeverity::Warning,
            &format!("`--{}` is deprecated in the Bazel 8.7 catalog", flag.name),
        ));
    }

    let mut statuses = Vec::new();
    if tagged(&flag.effect_tags, "NO_OP") {
        statuses.push("no-op");
    }
    if tagged(&flag.metadata_tags, "HIDDEN") {
        statuses.push("hidden");
    }
    if flag.documentation_category.as_deref() == Some("UNDOCUMENTED") {
        statuses.push("undocumented");
    }
    if !statuses.is_empty() {
        found.push(finding(
            document,
            range,
            DiagnosticSeverity::Information,
            &format!(
                "`--{}` is catalogued as {} by Bazel 8.7",
                flag.name,
                statuses.join(", ")
            ),
        ));
    }
}

fn tagged(tags: &[Box<str>], wanted: &str) -> bool {
    tags.iter().any(|tag| tag.as_ref() == wanted)
}

fn finding(
    document: &Document,
    range: Span,
    severity: DiagnosticSeverity,
    message: &str,
) -> Diagnostic {
    Diagnostic {
        range: span(document, range),
        severity: Some(severity),
        source: Some(SOURCE.to_owned()),
        message: message.to_owned().into(),
        ..Default::default()
    }
}

fn span(document: &Document, range: Span) -> Range {
    Range {
        start: document.line_index().position(document.text(), range.start),
        end: document.line_index().position(document.text(), range.end),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bazelrc::{Flag, FlagCatalog};
    use crate::index::IndexHandle;
    use lsp_types::Uri;

    fn documents(text: &str) -> (Documents, Uri) {
        let root = std::path::PathBuf::from("/ws");
        let mut documents = Documents::new(Some(root.clone()), IndexHandle::new());
        let uri: Uri = "file:///ws/.bazelrc".parse().unwrap();
        documents.set(uri.clone(), root.join(".bazelrc"), 1, text.to_owned());
        (documents, uri)
    }

    fn catalog() -> FlagCatalog {
        FlagCatalog::from_flags(
            "bazel 8.7.0",
            vec![
                Flag {
                    name: "jobs".into(),
                    commands: vec!["build".into()],
                    requires_value: true,
                    old_name: Some("job_count".into()),
                    ..Default::default()
                },
                Flag {
                    name: "keep_going".into(),
                    commands: vec!["build".into()],
                    has_negative_flag: true,
                    deprecation_warning: Some("use another flag".into()),
                    ..Default::default()
                },
            ],
        )
    }

    fn message_contains(finding: &Diagnostic, text: &str) -> bool {
        matches!(&finding.message, lsp_types::Message::String(message) if message.contains(text))
    }

    #[test]
    fn absence_is_qualified_and_build_settings_are_exempt() {
        let (documents, uri) = documents(
            "build --future=value --//settings:mode=value --@repo//settings:mode=value\n",
        );
        let found = diagnostics(
            documents.get(&uri).unwrap(),
            &documents,
            &ConfigurationSnapshot::default(),
            Some(&catalog()),
        );
        assert_eq!(found.len(), 1);
        assert!(message_contains(&found[0], "native flag catalog"));
    }

    #[test]
    fn exact_catalog_contradictions_are_errors() {
        let (documents, uri) =
            documents("startup --jobs\nbuild --jobs\nbuild --nokeep_going=value\n");
        let found = diagnostics(
            documents.get(&uri).unwrap(),
            &documents,
            &ConfigurationSnapshot::default(),
            Some(&catalog()),
        );
        assert_eq!(
            found
                .iter()
                .filter(|finding| finding.severity == Some(DiagnosticSeverity::Error))
                .count(),
            4
        );
        assert!(
            found
                .iter()
                .any(|finding| message_contains(finding, "use another flag"))
        );
    }

    #[test]
    fn config_findings_respect_open_buffer_declarations() {
        let text = "build:outer --config inner\nbuild --config external\nbuild:external --jobs=1\n";
        let (documents, uri) = documents(text);
        let found = diagnostics(
            documents.get(&uri).unwrap(),
            &documents,
            &ConfigurationSnapshot::default(),
            None,
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].severity, Some(DiagnosticSeverity::Error));
        assert!(message_contains(&found[0], "--config=name"));
    }

    #[test]
    fn external_config_names_are_qualified_warnings() {
        let (documents, uri) = documents("build --config=personal\n");
        let configuration = ConfigurationSnapshot {
            root: Some(std::sync::Arc::from(std::path::Path::new("/ws"))),
            ..ConfigurationSnapshot::default()
        };
        let found = diagnostics(
            documents.get(&uri).unwrap(),
            &documents,
            &configuration,
            None,
        );
        assert_eq!(found[0].severity, Some(DiagnosticSeverity::Warning));
        assert!(message_contains(&found[0], "workspace Bazelrc graph"));
    }

    #[test]
    fn unavailable_snapshots_do_not_claim_a_config_is_absent() {
        let (documents, uri) = documents("build --config=personal\n");
        assert!(
            diagnostics(
                documents.get(&uri).unwrap(),
                &documents,
                &ConfigurationSnapshot::default(),
                None,
            )
            .is_empty()
        );
    }
}
