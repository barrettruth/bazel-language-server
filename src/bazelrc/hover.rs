//! Snapshot-backed hover for Bazelrc structure and exact-catalog flags.

use std::collections::BTreeSet;
use std::path::Path;

use lsp_types::{Contents, Hover, MarkupContent, MarkupKind, Position, Range};

use super::native_options;
use super::syntax::{Directive, Span, Statement, Token};
use super::{
    ConfigurationSnapshot, ConfigurationView, Flag, FlagCatalog, FlagSpelling, ResolvedFlag,
    commands,
};
use crate::document::{Document, Documents};

#[must_use]
pub fn hover(
    document: &Document,
    documents: &Documents,
    configuration: &ConfigurationSnapshot,
    catalog: Option<&FlagCatalog>,
    root: Option<&Path>,
    position: Position,
) -> Option<Hover> {
    let offset = document.line_index().offset(document.text(), position);
    let view = ConfigurationView::for_document(document, documents, configuration);
    if let Some((declaration, occurrence)) = view.occurrence_at(document.path(), offset) {
        return config_hover(document, &view, declaration, occurrence);
    }
    for line in &document.bazelrc()?.lines {
        let Some(key) = line.key() else {
            continue;
        };
        if let Some(hover) = import_hover(document, configuration, root, line, offset) {
            return Some(hover);
        }
        let command = key
            .text
            .split_once(':')
            .map_or(key.text.as_str(), |(command, _)| command);
        if commands::NAMES.contains(&command)
            && let Some(command_range) = key.decoded_span(0..command.len())
            && contains(command_range, offset)
        {
            return Some(command_hover(document, command, command_range));
        }
        if key.text.contains(':') && !commands::accepts_config(command) {
            continue;
        }
        let Some(catalog) = catalog else { continue };
        for option in native_options::uses(line, catalog) {
            let token = if contains(option.option.range, offset) {
                option.option
            } else if option
                .value
                .is_some_and(|value| contains(value.range, offset))
            {
                option.value?
            } else {
                continue;
            };
            return Some(render(document, catalog, command, token, option.resolved?));
        }
    }
    None
}

fn command_hover(document: &Document, command: &str, span: Span) -> Hover {
    let text = match command {
        "startup" => {
            "startup\n\nNative client options read before a Bazel command is selected.".to_owned()
        }
        "always" => {
            "always\n\nRc-only scope consulted before `common` for every non-startup command."
                .to_owned()
        }
        "common" => {
            "common\n\nRc-only scope consulted for every non-startup command after `always`."
                .to_owned()
        }
        _ => format!(
            "{command}\n\nBazel 8.7 rc order: {}",
            commands::scopes(command).join(" → ")
        ),
    };
    plain_hover(document, span, text)
}

fn config_hover(
    document: &Document,
    view: &ConfigurationView<'_>,
    declaration: bool,
    occurrence: &super::index::ConfigSite,
) -> Option<Hover> {
    let declarations: BTreeSet<_> = view
        .applicable_declarations(&occurrence.command, &occurrence.name)
        .map(|site| site.command.as_ref())
        .collect();
    let expansions: BTreeSet<_> = view
        .references()
        .filter(|site| {
            site.owner.as_deref() == Some(occurrence.name.as_ref())
                && commands::applies(&occurrence.command, &site.command)
        })
        .map(|site| site.name.as_ref())
        .collect();
    let mut text = format!("--config={}\n\n", occurrence.name);
    field(
        &mut text,
        "Occurrence",
        Some(if declaration {
            "declaration"
        } else {
            "reference"
        }),
    );
    field(&mut text, "Current scope", Some(&occurrence.command));
    let declarations = declarations.into_iter().collect::<Vec<_>>().join(", ");
    field(
        &mut text,
        "Applicable declaration scopes",
        (!declarations.is_empty()).then_some(declarations.as_str()),
    );
    let expansions = expansions.into_iter().collect::<Vec<_>>().join(", ");
    field(
        &mut text,
        "Known expansions",
        (!expansions.is_empty()).then_some(expansions.as_str()),
    );
    text.push_str("Graph: workspace imports and open Bazelrc buffers");
    Some(plain_hover(document, occurrence.range, text))
}

fn import_hover(
    document: &Document,
    configuration: &ConfigurationSnapshot,
    root: Option<&Path>,
    line: &super::syntax::Line,
    offset: usize,
) -> Option<Hover> {
    let Some(Statement::Directive(directive)) = &line.statement else {
        return None;
    };
    let key = line.key()?;
    let (path, condition) = match directive {
        Directive::ConditionalImport(condition) => (line.tokens.get(2)?, Some(condition)),
        Directive::Import | Directive::TryImport => (line.tokens.get(1)?, None),
    };
    let hovered = if contains(key.range, offset) {
        key
    } else if condition.is_some() && contains(line.tokens[1].range, offset) {
        &line.tokens[1]
    } else if contains(path.range, offset) {
        path
    } else {
        return None;
    };
    let mut text = format!("{}\n\n", key.text);
    field(
        &mut text,
        "Mode",
        Some(match directive {
            Directive::Import => "required",
            Directive::TryImport => "optional",
            Directive::ConditionalImport(_) => "version-gated optional",
        }),
    );
    if let Some(condition) = condition {
        field(&mut text, "Condition", Some(&line.tokens[1].text));
        field(
            &mut text,
            "Bazel 8.7.0",
            Some(if condition.matches("8.7.0") {
                "active"
            } else {
                "inactive"
            }),
        );
    }
    if let Some(root) = root {
        let target = super::index::resolve_import(root, &path.text);
        field(&mut text, "Resolved path", target.to_str());
        let identity = configuration
            .identity(document.path())
            .unwrap_or_else(|| document.path());
        let published = configuration
            .imports
            .iter()
            .filter(|site| site.file.as_ref() == identity && site.target == target)
            .find(|site| site.range == path.range)
            .or_else(|| {
                configuration
                    .imports
                    .iter()
                    .find(|site| site.file.as_ref() == identity && site.target == target)
            });
        field(
            &mut text,
            "Published state",
            Some(match published {
                Some(site) if !site.active => "inactive",
                Some(site) if site.loaded.is_some() => "loaded",
                Some(_) => "not loaded",
                None => "not present in the saved import graph",
            }),
        );
    }
    Some(plain_hover(document, hovered.range, text))
}

fn plain_hover(document: &Document, span: Span, value: String) -> Hover {
    Hover {
        contents: Contents::MarkupContent(MarkupContent {
            kind: MarkupKind::PlainText,
            value,
        }),
        range: Some(range(document, span)),
    }
}

fn render(
    document: &Document,
    catalog: &FlagCatalog,
    command: &str,
    token: &Token,
    resolved: ResolvedFlag<'_>,
) -> Hover {
    let flag = resolved.flag;
    let mut text = format!("--{}\n", flag.name);
    if let Some(documentation) = &flag.documentation
        && !documentation.is_empty()
    {
        text.push('\n');
        text.push_str(documentation);
        text.push('\n');
    }
    field(&mut text, "Spelling", spelling(resolved.spelling));
    let alternates = alternate_spellings(flag);
    field(&mut text, "Alternate spellings", Some(&alternates));
    field(&mut text, "Type", flag.type_converter.as_deref());
    field(&mut text, "Default", flag.default_value.as_deref());
    let values = joined(&flag.enum_values);
    field(
        &mut text,
        "Values",
        (!values.is_empty()).then_some(values.as_str()),
    );
    let commands = joined(&flag.commands);
    field(&mut text, "Commands", Some(&commands));
    field(
        &mut text,
        "Current scope",
        Some(if catalog.supports_scope(flag, command) {
            "supported"
        } else {
            "not reported for this command"
        }),
    );
    if flag.allows_multiple {
        field(&mut text, "Repeatable", Some("yes"));
    }
    field(&mut text, "Deprecated", flag.deprecation_warning.as_deref());
    let expansions = joined(&flag.option_expansions);
    field(
        &mut text,
        "Expands to",
        (!expansions.is_empty()).then_some(expansions.as_str()),
    );
    let effects = joined(&flag.effect_tags);
    field(
        &mut text,
        "Effects",
        (!effects.is_empty()).then_some(effects.as_str()),
    );
    let metadata = joined(&flag.metadata_tags);
    field(
        &mut text,
        "Metadata",
        (!metadata.is_empty()).then_some(metadata.as_str()),
    );
    field(
        &mut text,
        "Category",
        flag.documentation_category.as_deref(),
    );
    field(&mut text, "Catalog", Some(catalog.reported()));
    Hover {
        contents: Contents::MarkupContent(MarkupContent {
            kind: MarkupKind::PlainText,
            value: text,
        }),
        range: Some(range(document, token.range)),
    }
}

fn alternate_spellings(flag: &Flag) -> String {
    let mut spellings = Vec::new();
    if flag.has_negative_flag {
        spellings.push(format!("--no{}", flag.name));
    }
    if let Some(abbreviation) = &flag.abbreviation {
        spellings.push(format!("-{abbreviation}"));
        if flag.has_negative_flag {
            spellings.push(format!("-{abbreviation}-"));
        }
    }
    if let Some(old_name) = &flag.old_name {
        spellings.push(format!("--{old_name} (deprecated name)"));
    }
    spellings.join(", ")
}

const fn spelling(spelling: FlagSpelling) -> Option<&'static str> {
    match spelling {
        FlagSpelling::Canonical => None,
        FlagSpelling::Negative => Some("negative form"),
        FlagSpelling::Abbreviation => Some("abbreviation"),
        FlagSpelling::NegativeAbbreviation => Some("negative abbreviation"),
        FlagSpelling::OldName => Some("deprecated name"),
        FlagSpelling::NegativeOldName => Some("negative form of deprecated name"),
    }
}

fn field(text: &mut String, name: &str, value: Option<&str>) {
    if let Some(value) = value.filter(|value| !value.is_empty()) {
        use std::fmt::Write as _;
        let _ = writeln!(text, "{name}: {value}");
    }
}

fn joined(values: &[Box<str>]) -> String {
    values
        .iter()
        .map(AsRef::as_ref)
        .collect::<Vec<&str>>()
        .join(", ")
}

fn contains(span: Span, offset: usize) -> bool {
    span.start <= offset && offset <= span.end
}

fn range(document: &Document, span: Span) -> Range {
    Range {
        start: document.line_index().position(document.text(), span.start),
        end: document.line_index().position(document.text(), span.end),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::index::IndexHandle;
    use lsp_types::Uri;

    fn documents(text: &str) -> (Documents, Uri, ConfigurationSnapshot) {
        let root = std::path::PathBuf::from("/ws");
        let mut documents = Documents::new(Some(root.clone()), IndexHandle::new());
        let uri: Uri = "file:///ws/.bazelrc".parse().unwrap();
        documents.set(uri.clone(), root.join(".bazelrc"), 1, text.to_owned());
        let configuration = ConfigurationSnapshot {
            root: Some(Arc::from(root.as_path())),
            ..ConfigurationSnapshot::default()
        };
        (documents, uri, configuration)
    }

    #[test]
    fn a_separate_value_hovers_the_exact_catalog_flag() {
        let flag = Flag {
            name: "jobs".into(),
            documentation: Some("Number of concurrent jobs".into()),
            commands: vec!["build".into()],
            requires_value: true,
            default_value: Some("auto".into()),
            type_converter: Some("Jobs".into()),
            ..Default::default()
        };
        let catalog = FlagCatalog::from_flags("bazel 8.7.0", vec![flag]);
        let text = "build --jobs 4\n";
        let (documents, uri, configuration) = documents(text);
        let document = documents.get(&uri).unwrap();
        let position = document
            .line_index()
            .position(text, text.find('4').unwrap());
        let hover = hover(
            document,
            &documents,
            &configuration,
            Some(&catalog),
            Some(Path::new("/ws")),
            position,
        )
        .unwrap();
        let Contents::MarkupContent(contents) = hover.contents else {
            panic!("plain hover")
        };
        assert!(contents.value.contains("--jobs"));
        assert!(contents.value.contains("Default: auto"));
        assert!(contents.value.contains("Catalog: bazel 8.7.0"));
    }

    #[test]
    fn structural_hover_does_not_require_a_flag_catalog() {
        let text = "build:dev --config=nested\nbuild:nested --jobs=1\n\
                    try-import-if-bazel-version >=8.7.0 config/plain\n";
        let (documents, uri, mut configuration) = documents(text);
        let document = documents.get(&uri).unwrap();

        let config_position = document
            .line_index()
            .position(text, text.find("dev").unwrap());
        let card = hover(
            document,
            &documents,
            &configuration,
            None,
            Some(Path::new("/ws")),
            config_position,
        )
        .unwrap();
        let Contents::MarkupContent(contents) = card.contents else {
            panic!("plain hover")
        };
        assert!(contents.value.contains("Known expansions: nested"));

        let command_position = document.line_index().position(text, 1);
        let card = hover(
            document,
            &documents,
            &configuration,
            None,
            Some(Path::new("/ws")),
            command_position,
        )
        .unwrap();
        let Contents::MarkupContent(contents) = card.contents else {
            panic!("plain hover")
        };
        assert!(contents.value.contains("always → common → build"));

        let parsed = document.bazelrc().unwrap();
        let path = &parsed.lines[2].tokens[2];
        configuration.imports.push(super::super::ImportSite {
            file: Arc::from(document.path()),
            range: path.range,
            target: Path::new("/ws/config/plain").to_path_buf(),
            loaded: Some(Arc::from(Path::new("/ws/config/plain"))),
            active: true,
        });
        let import_position = document
            .line_index()
            .position(text, text.find("config/plain").unwrap());
        let card = hover(
            document,
            &documents,
            &configuration,
            None,
            Some(Path::new("/ws")),
            import_position,
        )
        .unwrap();
        let Contents::MarkupContent(contents) = card.contents else {
            panic!("plain hover")
        };
        assert!(contents.value.contains("Published state: loaded"));
    }
}
