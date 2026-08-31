//! Bazel 8.7 named-configuration expansion findings.

use std::collections::{BTreeSet, HashMap};

use rustc_hash::{FxHashMap, FxHashSet};

use super::ConfigurationView;
use super::commands;
use super::index::ConfigSite;

const DEEP_CHAIN: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

pub struct Finding {
    pub site: ConfigSite,
    pub severity: Severity,
    pub message: String,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Cycle,
    Repeated,
    Deep,
}

struct Graph<'a> {
    definitions: FxHashSet<&'a str>,
    edges: FxHashMap<&'a str, Vec<&'a ConfigSite>>,
    roots: Vec<&'a ConfigSite>,
}

#[must_use]
pub fn findings(view: &ConfigurationView<'_>) -> Vec<Finding> {
    if !view.ready() {
        return Vec::new();
    }
    let mut findings = Vec::new();
    let mut seen = BTreeSet::new();
    for command in commands::NAMES
        .iter()
        .copied()
        .filter(|command| !matches!(*command, "always" | "common" | "startup"))
    {
        let graph = Graph::new(view, command);
        graph.cycles(&mut findings, &mut seen);
        graph.repetitions(&mut findings, &mut seen);
        graph.deep_chains(&mut findings, &mut seen);
    }
    findings.sort_unstable_by(|left, right| {
        left.site
            .file
            .cmp(&right.site.file)
            .then_with(|| left.site.range.start.cmp(&right.site.range.start))
            .then_with(|| left.message.cmp(&right.message))
    });
    findings
}

impl<'a> Graph<'a> {
    fn new(view: &'a ConfigurationView<'_>, command: &str) -> Self {
        let definitions: FxHashSet<_> = view
            .declarations()
            .filter(|site| commands::applies(command, &site.command))
            .map(|site| site.name.as_ref())
            .collect();
        let mut edges: FxHashMap<&str, Vec<&ConfigSite>> = FxHashMap::default();
        let mut roots = Vec::new();
        for site in view
            .references()
            .filter(|site| commands::applies(command, &site.command))
        {
            if let Some(owner) = site.owner.as_deref() {
                edges.entry(owner).or_default().push(site);
            } else {
                roots.push(site);
            }
        }
        Self {
            definitions,
            edges,
            roots,
        }
    }

    fn cycles(&self, findings: &mut Vec<Finding>, seen: &mut BTreeSet<FindingKey>) {
        for (owner, edges) in &self.edges {
            for edge in edges {
                if !self.definitions.contains(edge.name.as_ref()) {
                    continue;
                }
                let mut visited = FxHashSet::default();
                let Some(mut path) = self.path(edge.name.as_ref(), owner, &mut visited) else {
                    continue;
                };
                path.insert(0, owner);
                push(
                    findings,
                    seen,
                    edge,
                    Kind::Cycle,
                    Severity::Error,
                    format!("configuration expansion cycle: {}", display_chain(&path)),
                );
            }
        }
    }

    fn path(
        &self,
        from: &'a str,
        target: &str,
        visited: &mut FxHashSet<&'a str>,
    ) -> Option<Vec<&'a str>> {
        if from == target {
            return Some(vec![from]);
        }
        if !visited.insert(from) {
            return None;
        }
        for edge in self.edges.get(from).into_iter().flatten() {
            if !self.definitions.contains(edge.name.as_ref()) {
                continue;
            }
            if let Some(mut path) = self.path(edge.name.as_ref(), target, visited) {
                path.insert(0, from);
                return Some(path);
            }
        }
        None
    }

    fn repetitions(&self, findings: &mut Vec<Finding>, seen: &mut BTreeSet<FindingKey>) {
        let mut expanded = FxHashSet::default();
        for root in &self.roots {
            self.expand(root, &mut Vec::new(), &mut expanded, findings, seen);
        }
    }

    fn expand(
        &self,
        edge: &ConfigSite,
        active: &mut Vec<Box<str>>,
        expanded: &mut FxHashSet<Box<str>>,
        findings: &mut Vec<Finding>,
        seen: &mut BTreeSet<FindingKey>,
    ) {
        if !self.definitions.contains(edge.name.as_ref())
            || active.iter().any(|name| name == &edge.name)
        {
            return;
        }
        if !expanded.insert(edge.name.clone()) {
            push(
                findings,
                seen,
                edge,
                Kind::Repeated,
                Severity::Warning,
                format!(
                    "configuration `{}` is expanded more than once; repeatable flags are applied each time",
                    edge.name
                ),
            );
        }
        active.push(edge.name.clone());
        for child in self.edges.get(edge.name.as_ref()).into_iter().flatten() {
            self.expand(child, active, expanded, findings, seen);
        }
        active.pop();
    }

    fn deep_chains(&self, findings: &mut Vec<Finding>, seen: &mut BTreeSet<FindingKey>) {
        let mut memo = HashMap::new();
        for root in &self.roots {
            if !self.definitions.contains(root.name.as_ref()) {
                continue;
            }
            let Ok(path) = self.longest(root.name.as_ref(), &mut Vec::new(), &mut memo) else {
                continue;
            };
            if path.len() + 1 < DEEP_CHAIN {
                continue;
            }
            let site = path.last().copied().unwrap_or(root);
            let mut names = Vec::with_capacity(path.len() + 1);
            names.push(root.name.as_ref());
            names.extend(path.iter().map(|site| site.name.as_ref()));
            push(
                findings,
                seen,
                site,
                Kind::Deep,
                Severity::Warning,
                format!(
                    "configuration expansion chain has {} configs: {}",
                    names.len(),
                    display_chain(&names)
                ),
            );
        }
    }

    fn longest(
        &self,
        name: &'a str,
        active: &mut Vec<&'a str>,
        memo: &mut HashMap<&'a str, Vec<&'a ConfigSite>>,
    ) -> Result<Vec<&'a ConfigSite>, ()> {
        if active.contains(&name) {
            return Err(());
        }
        if let Some(path) = memo.get(name) {
            return Ok(path.clone());
        }
        active.push(name);
        let mut longest = Vec::new();
        for edge in self.edges.get(name).into_iter().flatten() {
            if !self.definitions.contains(edge.name.as_ref()) {
                continue;
            }
            let mut candidate = vec![*edge];
            candidate.extend(self.longest(edge.name.as_ref(), active, memo)?);
            if candidate.len() > longest.len() {
                longest = candidate;
            }
        }
        active.pop();
        memo.insert(name, longest.clone());
        Ok(longest)
    }
}

type FindingKey = (std::path::PathBuf, usize, usize, Kind);

fn push(
    findings: &mut Vec<Finding>,
    seen: &mut BTreeSet<FindingKey>,
    site: &ConfigSite,
    kind: Kind,
    severity: Severity,
    message: String,
) {
    if seen.insert((
        site.file.to_path_buf(),
        site.range.start,
        site.range.end,
        kind,
    )) {
        findings.push(Finding {
            site: site.clone(),
            severity,
            message,
        });
    }
}

fn display_chain(names: &[&str]) -> String {
    names
        .iter()
        .map(|name| format!("`{name}`"))
        .collect::<Vec<_>>()
        .join(" → ")
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use lsp_types::Uri;

    use super::*;
    use crate::bazelrc::ConfigurationSnapshot;
    use crate::document::Documents;
    use crate::index::IndexHandle;

    fn analyze(text: &str) -> Vec<Finding> {
        let root = std::path::PathBuf::from("/ws");
        let mut documents = Documents::new(Some(root.clone()), IndexHandle::new());
        let uri: Uri = "file:///ws/.bazelrc".parse().unwrap();
        documents.set(uri, root.join(".bazelrc"), 1, text.to_owned());
        let snapshot = ConfigurationSnapshot {
            root: Some(Arc::from(root.as_path())),
            ..ConfigurationSnapshot::default()
        };
        findings(&ConfigurationView::new(&documents, &snapshot))
    }

    #[test]
    fn cycles_are_branch_local_and_repetition_is_not_a_cycle() {
        let found = analyze(
            "build:a --config=b\nbuild:b --config=a\n\
             build:reuse --config=leaf\nbuild:leaf --jobs=1\n\
             build:left --config=reuse\nbuild:right --config=reuse\n\
             build --config=left --config=right\n",
        );
        assert_eq!(
            found
                .iter()
                .filter(|finding| finding.severity == Severity::Error)
                .count(),
            2
        );
        assert_eq!(
            found
                .iter()
                .filter(|finding| finding.message.contains("more than once"))
                .count(),
            2
        );
    }

    #[test]
    fn ten_configs_is_the_deep_chain_boundary() {
        let mut text = String::new();
        for number in 0..9 {
            text.push_str(&format!("build:c{number} --config=c{}\n", number + 1));
        }
        text.push_str("build:c9 --jobs=1\nbuild --config=c0\n");
        let found = analyze(&text);
        let deep = found
            .iter()
            .find(|finding| finding.message.contains("10 configs"))
            .expect("deep-chain warning");
        assert_eq!(deep.site.name.as_ref(), "c9");
    }
}
