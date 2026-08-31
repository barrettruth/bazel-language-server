//! Structural completion plus exact-catalog native flags.

use std::collections::{BTreeMap, BTreeSet};

use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionResponse, Position, Range, TextEdit,
};

use super::commands;
use super::native_options;
use super::syntax::{Statement, config_references};
use super::{ConfigurationSnapshot, ConfigurationView, Flag, FlagCatalog};
use crate::document::{Document, Documents};

const DIRECTIVES: &[(&str, &str)] = &[
    ("import", "Required Bazelrc import"),
    ("try-import", "Optional Bazelrc import"),
    (
        "try-import-if-bazel-version",
        "Version-gated optional Bazelrc import",
    ),
];

#[must_use]
pub fn completions(
    document: &Document,
    documents: &Documents,
    configuration: &ConfigurationSnapshot,
    catalog: Option<&FlagCatalog>,
    position: Position,
) -> CompletionResponse {
    let offset = document.line_index().offset(document.text(), position);
    let Some(line) = document.bazelrc().and_then(|parsed| {
        parsed
            .lines
            .iter()
            .find(|line| line.range.start <= offset && offset <= line.range.end)
    }) else {
        return Vec::new().into();
    };
    if line.comment.is_some_and(|comment| offset >= comment.start) {
        return Vec::new().into();
    }
    let Some(key) = line.key() else {
        return command_items().into();
    };
    if offset <= key.range.end {
        return command_items().into();
    }
    if matches!(
        line.statement,
        Some(Statement::Directive(_) | Statement::InvalidDirective)
    ) {
        return Vec::new().into();
    }
    let command = key
        .text
        .split_once(':')
        .map_or(key.text.as_str(), |(command, _)| command);
    let trailing_config = line
        .options()
        .last()
        .filter(|option| option.text == "--config" && offset >= option.range.end);
    if key.text.contains(':') && trailing_config.is_some_and(|option| offset > option.range.end) {
        return Vec::new().into();
    }
    let completing_config = config_references(line)
        .iter()
        .any(|reference| reference.range.start <= offset && offset <= reference.range.end)
        || (!key.text.contains(':') && trailing_config.is_some());
    if completing_config {
        if commands::accepts_config(command) {
            config_items(&ConfigurationView::new(documents, configuration), command).into()
        } else {
            Vec::new().into()
        }
    } else if !commands::NAMES.contains(&command) {
        Vec::new().into()
    } else if let Some(catalog) = catalog {
        if let Some(context) = enum_value_context(line, catalog, command, offset) {
            enum_items(document, context).into()
        } else if completing_native_flag(line, catalog, offset, document.text()) {
            flag_items(catalog, command).into()
        } else {
            Vec::new().into()
        }
    } else {
        Vec::new().into()
    }
}

struct EnumValueContext<'a> {
    flag: &'a Flag,
    replacement: super::syntax::Span,
    option: Option<&'a str>,
}

fn enum_value_context<'a>(
    line: &'a super::syntax::Line,
    catalog: &'a FlagCatalog,
    command: &str,
    offset: usize,
) -> Option<EnumValueContext<'a>> {
    for usage in native_options::uses(line, catalog) {
        let Some(resolved) = usage.resolved else {
            continue;
        };
        if resolved.flag.enum_values.is_empty()
            || !catalog.supports_scope(resolved.flag, command)
            || matches!(
                resolved.spelling,
                super::FlagSpelling::Negative | super::FlagSpelling::NegativeOldName
            )
        {
            continue;
        }
        if let Some((option, _)) = usage.option.text.split_once('=') {
            let equals = usage.option.text.find('=')?;
            let equals = usage.option.decoded_span(equals..equals + 1)?;
            if equals.end <= offset && offset <= usage.option.range.end {
                return Some(EnumValueContext {
                    flag: resolved.flag,
                    replacement: usage.option.range,
                    option: Some(option),
                });
            }
        } else if let Some(value) = usage.value {
            if value.range.start <= offset && offset <= value.range.end {
                return Some(EnumValueContext {
                    flag: resolved.flag,
                    replacement: value.range,
                    option: None,
                });
            }
        } else if offset > usage.option.range.end && offset <= line.range.end {
            return Some(EnumValueContext {
                flag: resolved.flag,
                replacement: super::syntax::Span::new(offset, offset),
                option: None,
            });
        }
    }
    None
}

fn enum_items(document: &Document, context: EnumValueContext<'_>) -> Vec<CompletionItem> {
    let range = Range {
        start: document
            .line_index()
            .position(document.text(), context.replacement.start),
        end: document
            .line_index()
            .position(document.text(), context.replacement.end),
    };
    if range.start.line != range.end.line {
        return Vec::new();
    }
    context
        .flag
        .enum_values
        .iter()
        .filter_map(|raw| {
            let value = super::syntax::quote_token(raw)?;
            let new_text = context
                .option
                .map_or(value.clone(), |option| format!("{option}={value}"));
            Some(CompletionItem {
                label: raw.to_string(),
                kind: Some(CompletionItemKind::EnumMember),
                detail: Some(format!("Value for `--{}`", context.flag.name)),
                text_edit: Some(TextEdit { range, new_text }.into()),
                ..Default::default()
            })
        })
        .collect()
}

fn completing_native_flag(
    line: &super::syntax::Line,
    catalog: &FlagCatalog,
    offset: usize,
    source: &str,
) -> bool {
    let Some(current) = line
        .options()
        .iter()
        .find(|option| option.range.start <= offset && offset <= option.range.end)
    else {
        return true;
    };
    if native_options::uses(line, catalog).iter().any(|option| {
        option
            .value
            .is_some_and(|value| std::ptr::eq(value, current))
    }) {
        return false;
    }
    if !current.text.starts_with('-')
        || current.text.starts_with("--//")
        || current.text.starts_with("--@")
        || current.text.starts_with("--no//")
        || current.text.starts_with("--no@")
    {
        return false;
    }
    let Some(equals) = current.text.find('=') else {
        return true;
    };
    source.get(current.range.start..current.range.end) == Some(current.text.as_str())
        && offset <= current.range.start + equals
}

fn flag_items(catalog: &FlagCatalog, command: &str) -> Vec<CompletionItem> {
    let mut items = Vec::new();
    for flag in catalog
        .flags()
        .filter(|flag| catalog.supports_scope(flag, command))
        .filter(|flag| visible(flag))
    {
        push_flag(&mut items, flag, &format!("--{}", flag.name));
        if flag.has_negative_flag {
            push_flag(&mut items, flag, &format!("--no{}", flag.name));
        }
        if let Some(abbreviation) = &flag.abbreviation {
            push_flag(&mut items, flag, &format!("-{abbreviation}"));
            if flag.has_negative_flag {
                push_flag(&mut items, flag, &format!("-{abbreviation}-"));
            }
        }
    }
    items.sort_by(|left, right| left.label.cmp(&right.label));
    items
}

fn visible(flag: &Flag) -> bool {
    flag.documentation_category.as_deref() != Some("UNDOCUMENTED")
        && !flag
            .metadata_tags
            .iter()
            .any(|tag| tag.as_ref() == "HIDDEN")
        && !flag.effect_tags.iter().any(|tag| tag.as_ref() == "NO_OP")
}

fn push_flag(items: &mut Vec<CompletionItem>, flag: &Flag, spelling: &str) {
    let detail = flag.type_converter.as_deref().map_or_else(
        || "Bazel 8.7 flag".to_owned(),
        |value| format!("Bazel 8.7 flag · {value}"),
    );
    items.push(CompletionItem {
        label: spelling.to_owned(),
        kind: Some(CompletionItemKind::Property),
        tags: flag
            .deprecation_warning
            .as_ref()
            .map(|_| ())
            .or_else(|| {
                flag.metadata_tags
                    .iter()
                    .any(|tag| tag.as_ref() == "DEPRECATED")
                    .then_some(())
            })
            .map(|()| vec![lsp_types::CompletionItemTag::Deprecated]),
        detail: Some(detail),
        documentation: flag
            .documentation
            .as_ref()
            .map(|documentation| documentation.to_string().into()),
        insert_text: (flag.requires_value && spelling.starts_with("--"))
            .then(|| format!("{spelling}=")),
        ..Default::default()
    });
}

fn command_items() -> Vec<CompletionItem> {
    let mut items: Vec<_> = commands::NAMES
        .iter()
        .map(|name| CompletionItem {
            label: (*name).to_owned(),
            kind: Some(CompletionItemKind::Keyword),
            detail: Some(if matches!(*name, "always" | "common" | "startup") {
                "Bazel 8.7 rc scope".to_owned()
            } else {
                "Bazel 8.7 command".to_owned()
            }),
            ..Default::default()
        })
        .collect();
    items.extend(DIRECTIVES.iter().map(|(name, detail)| CompletionItem {
        label: (*name).to_owned(),
        kind: Some(CompletionItemKind::Keyword),
        detail: Some((*detail).to_owned()),
        ..Default::default()
    }));
    items
}

fn config_items(configuration: &ConfigurationView, command: &str) -> Vec<CompletionItem> {
    let mut found: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for site in configuration
        .declarations()
        .filter(|site| commands::applies(command, &site.command))
    {
        found
            .entry(site.name.to_string())
            .or_default()
            .insert(site.command.to_string());
    }
    found
        .into_iter()
        .map(|(name, commands)| CompletionItem {
            label: name,
            kind: Some(CompletionItemKind::Reference),
            detail: Some(format!(
                "Bazelrc config ({})",
                commands.into_iter().collect::<Vec<_>>().join(", ")
            )),
            ..Default::default()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::IndexHandle;
    use lsp_types::Uri;

    fn flag(name: &str, commands: &[&str]) -> Flag {
        Flag {
            name: name.into(),
            commands: commands.iter().map(|command| (*command).into()).collect(),
            ..Default::default()
        }
    }

    fn complete_items(text: &str, catalog: &FlagCatalog) -> Vec<CompletionItem> {
        let root = std::path::PathBuf::from("/ws");
        let mut documents = Documents::new(Some(root.clone()), IndexHandle::new());
        let uri: Uri = "file:///ws/.bazelrc".parse().unwrap();
        documents.set(uri.clone(), root.join(".bazelrc"), 1, text.to_owned());
        let document = documents.get(&uri).unwrap();
        let response = completions(
            document,
            &documents,
            &ConfigurationSnapshot::default(),
            Some(catalog),
            document.line_index().position(text, text.len()),
        );
        let CompletionResponse::CompletionItemList(items) = response else {
            panic!("array completion")
        };
        items
    }

    fn complete(text: &str, catalog: &FlagCatalog) -> Vec<String> {
        complete_items(text, catalog)
            .into_iter()
            .map(|item| item.label)
            .collect()
    }

    #[test]
    fn command_scope_controls_native_flags_and_negative_spellings() {
        let mut test = flag("test_output", &["test"]);
        test.has_negative_flag = true;
        let catalog =
            FlagCatalog::from_flags("bazel 8.7.0", vec![flag("jobs", &["build", "test"]), test]);
        let labels = complete("build ", &catalog);
        assert!(labels.contains(&"--jobs".to_owned()));
        assert!(!labels.contains(&"--test_output".to_owned()));
        let labels = complete("test ", &catalog);
        assert!(labels.contains(&"--test_output".to_owned()));
        assert!(labels.contains(&"--notest_output".to_owned()));
    }

    #[test]
    fn always_is_the_intersection_and_common_is_the_union() {
        let catalog = FlagCatalog::from_flags(
            "bazel 8.7.0",
            vec![
                flag("shared", &["build", "test"]),
                flag("test_only", &["test"]),
            ],
        );
        assert_eq!(complete("always ", &catalog), vec!["--shared"]);
        assert_eq!(
            complete("common ", &catalog),
            vec!["--shared", "--test_only"]
        );
    }

    #[test]
    fn values_and_build_settings_do_not_offer_native_flags() {
        let mut jobs = flag("jobs", &["build"]);
        jobs.requires_value = true;
        let catalog = FlagCatalog::from_flags("bazel 8.7.0", vec![jobs]);
        assert!(complete("build --jobs=", &catalog).is_empty());
        assert!(complete("build --//settings:mode=", &catalog).is_empty());
    }

    #[test]
    fn enum_values_replace_joined_and_separate_spellings_exactly() {
        let mut mode = flag("compilation_mode", &["build"]);
        mode.requires_value = true;
        mode.old_name = Some("cpu_mode".into());
        mode.enum_values = vec!["fastbuild".into(), "dbg".into()];
        let catalog = FlagCatalog::from_flags("bazel 8.7.0", vec![mode]);

        let joined = complete_items("build --cpu_mode=fa", &catalog);
        assert_eq!(
            joined
                .iter()
                .map(|item| item.label.as_str())
                .collect::<Vec<_>>(),
            ["fastbuild", "dbg"]
        );
        let Some(lsp_types::CompletionItemTextEdit::TextEdit(edit)) = &joined[0].text_edit else {
            panic!("plain text edit")
        };
        assert_eq!(edit.new_text, "--cpu_mode=\"fastbuild\"");
        assert_eq!(edit.range.start.character, 6);
        assert_eq!(edit.range.end.character, 19);

        let separate = complete_items("build --compilation_mode fa", &catalog);
        let Some(lsp_types::CompletionItemTextEdit::TextEdit(edit)) = &separate[1].text_edit else {
            panic!("plain text edit")
        };
        assert_eq!(edit.new_text, "\"dbg\"");
        assert_eq!(edit.range.start.character, 25);
        assert_eq!(edit.range.end.character, 27);
        assert!(complete("startup --compilation_mode=", &catalog).is_empty());
    }

    #[test]
    fn config_completion_uses_only_effective_declarations() {
        let catalog = FlagCatalog::from_flags("bazel 8.7.0", Vec::new());
        let labels = complete(
            "build:empty\nstartup:dev --host_jvm_args=-Xmx1g\n\
             future:dev --x\nbuild:present --define=x=1\nbuild --config=",
            &catalog,
        );
        assert_eq!(labels, vec!["present"]);
        assert!(complete("startup --config=", &catalog).is_empty());
        assert!(complete("build:outer --config ", &catalog).is_empty());
    }
}
