//! Exact-catalog hover for native Bazel flags.

use lsp_types::{Contents, Hover, MarkupContent, MarkupKind, Position, Range};

use super::native_options;
use super::syntax::{Span, Token};
use super::{Flag, FlagCatalog, FlagSpelling, ResolvedFlag};
use crate::document::Document;

#[must_use]
pub fn hover(document: &Document, catalog: &FlagCatalog, position: Position) -> Option<Hover> {
    let offset = document.line_index().offset(document.text(), position);
    for line in &document.bazelrc()?.lines {
        let Some(key) = line.key() else {
            continue;
        };
        let command = key
            .text
            .split_once(':')
            .map_or(key.text.as_str(), |(command, _)| command);
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
    use super::*;

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
        let document = Document::versioned(
            "/ws/.bazelrc".into(),
            1,
            text.to_owned(),
            Some(std::path::Path::new("/ws")),
        );
        let position = document
            .line_index()
            .position(text, text.find('4').unwrap());
        let hover = hover(&document, &catalog, position).unwrap();
        let Contents::MarkupContent(contents) = hover.contents else {
            panic!("plain hover")
        };
        assert!(contents.value.contains("--jobs"));
        assert!(contents.value.contains("Default: auto"));
        assert!(contents.value.contains("Catalog: bazel 8.7.0"));
    }
}
