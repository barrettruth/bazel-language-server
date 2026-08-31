//! Lossless Bazel 8.7 rc tokenization.
//!
//! Bazel's native client reads these files before the Java server starts. The
//! grammar is consequently the C++ tokenizer's, not Starlark's or a shell's.

use std::cmp::Ordering;
use std::ops::Range;

/// A contiguous range in the physical UTF-8 source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    #[must_use]
    pub const fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }
}

/// One token after Bazel's quote and escape processing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub text: String,
    pub range: Span,
    origins: Vec<Span>,
}

impl Token {
    /// The physical source covering a nonempty range of decoded token bytes.
    #[must_use]
    pub fn decoded_span(&self, range: Range<usize>) -> Option<Span> {
        if range.is_empty() || range.end > self.origins.len() {
            return None;
        }
        Some(Span::new(
            self.origins.get(range.start)?.start,
            self.origins.get(range.end - 1)?.end,
        ))
    }
}

/// A comparison in `try-import-if-bazel-version`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Comparison {
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Equal,
    NotEqual,
    Compatible,
}

/// One parsed version condition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionCondition {
    comparison: Comparison,
    version: SemanticVersion,
    compatible_upper: Option<SemanticVersion>,
}

impl VersionCondition {
    /// Parse the single condition token Bazel 8.7 accepts.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let (comparison, version) = [
            ("<=", Comparison::LessEqual),
            (">=", Comparison::GreaterEqual),
            ("==", Comparison::Equal),
            ("!=", Comparison::NotEqual),
            ("<", Comparison::Less),
            (">", Comparison::Greater),
            ("~", Comparison::Compatible),
        ]
        .into_iter()
        .find_map(|(prefix, comparison)| {
            text.strip_prefix(prefix)
                .map(|version| (comparison, version))
        })?;
        if version.is_empty() {
            return None;
        }

        if comparison == Comparison::Compatible {
            let (version, precision) = SemanticVersion::parse_partial(version)?;
            let upper = if precision == 1 {
                SemanticVersion::plain(version.major.checked_add(1)?, 0, 0)
            } else {
                SemanticVersion::plain(version.major, version.minor.checked_add(1)?, 0)
            };
            Some(Self {
                comparison,
                version,
                compatible_upper: Some(upper),
            })
        } else {
            Some(Self {
                comparison,
                version: SemanticVersion::parse_full(version)?,
                compatible_upper: None,
            })
        }
    }

    /// Whether `version` satisfies this condition.
    #[must_use]
    pub fn matches(&self, version: &str) -> bool {
        let Some(version) = SemanticVersion::parse_full(version) else {
            return false;
        };
        match self.comparison {
            Comparison::Less => version < self.version,
            Comparison::LessEqual => version <= self.version,
            Comparison::Greater => version > self.version,
            Comparison::GreaterEqual => version >= self.version,
            Comparison::Equal => version == self.version,
            Comparison::NotEqual => version != self.version,
            Comparison::Compatible => {
                version >= self.version
                    && self
                        .compatible_upper
                        .as_ref()
                        .is_some_and(|upper| version < *upper)
            }
        }
    }

    #[must_use]
    pub const fn comparison(&self) -> Comparison {
        self.comparison
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SemanticVersion {
    major: u64,
    minor: u64,
    patch: u64,
    prerelease: Vec<Identifier>,
}

impl SemanticVersion {
    const fn plain(major: u64, minor: u64, patch: u64) -> Self {
        Self {
            major,
            minor,
            patch,
            prerelease: Vec::new(),
        }
    }

    fn parse_full(text: &str) -> Option<Self> {
        let (core, prerelease) = split_suffixes(text)?;
        let mut fields = core.split('.');
        let major = number(fields.next()?)?;
        let minor = number(fields.next()?)?;
        let patch = number(fields.next()?)?;
        if fields.next().is_some() {
            return None;
        }
        Some(Self {
            major,
            minor,
            patch,
            prerelease: parse_prerelease(prerelease)?,
        })
    }

    fn parse_partial(text: &str) -> Option<(Self, usize)> {
        let (core, prerelease) = split_suffixes(text)?;
        let fields: Vec<_> = core.split('.').collect();
        if fields.is_empty() || fields.len() > 3 || fields.iter().any(|field| field.is_empty()) {
            return None;
        }
        if fields.len() < 3 && prerelease.is_some() {
            return None;
        }
        let major = number(fields[0])?;
        let minor = fields.get(1).map_or(Some(0), |field| number(field))?;
        let patch = fields.get(2).map_or(Some(0), |field| number(field))?;
        Some((
            Self {
                major,
                minor,
                patch,
                prerelease: parse_prerelease(prerelease)?,
            },
            fields.len(),
        ))
    }
}

impl Ord for SemanticVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        self.major
            .cmp(&other.major)
            .then_with(|| self.minor.cmp(&other.minor))
            .then_with(|| self.patch.cmp(&other.patch))
            .then_with(|| compare_prerelease(&self.prerelease, &other.prerelease))
    }
}

impl PartialOrd for SemanticVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Identifier {
    Numeric(String),
    Text(String),
}

fn split_suffixes(text: &str) -> Option<(&str, Option<&str>)> {
    let (without_build, _) = text
        .split_once('+')
        .map_or((text, None), |(head, tail)| (head, Some(tail)));
    if text.contains('+') {
        let build = text.split_once('+')?.1;
        if !valid_identifiers(build, false) || build.contains('+') {
            return None;
        }
    }
    let (core, prerelease) = without_build
        .split_once('-')
        .map_or((without_build, None), |(core, prerelease)| {
            (core, Some(prerelease))
        });
    Some((core, prerelease))
}

fn parse_prerelease(text: Option<&str>) -> Option<Vec<Identifier>> {
    let Some(text) = text else {
        return Some(Vec::new());
    };
    if !valid_identifiers(text, true) {
        return None;
    }
    text.split('.')
        .map(|part| {
            if part.bytes().all(|byte| byte.is_ascii_digit()) {
                if part.len() > 1 && part.starts_with('0') {
                    return None;
                }
                Some(Identifier::Numeric(part.to_owned()))
            } else {
                Some(Identifier::Text(part.to_owned()))
            }
        })
        .collect()
}

fn valid_identifiers(text: &str, prerelease: bool) -> bool {
    !text.is_empty()
        && text.split('.').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                && (!prerelease
                    || !part.bytes().all(|byte| byte.is_ascii_digit())
                    || part == "0"
                    || !part.starts_with('0'))
        })
}

fn number(text: &str) -> Option<u64> {
    if text.is_empty()
        || !text.bytes().all(|byte| byte.is_ascii_digit())
        || text.len() > 1 && text.starts_with('0')
    {
        return None;
    }
    text.parse().ok()
}

fn compare_prerelease(left: &[Identifier], right: &[Identifier]) -> Ordering {
    match (left.is_empty(), right.is_empty()) {
        (true, true) => return Ordering::Equal,
        (true, false) => return Ordering::Greater,
        (false, true) => return Ordering::Less,
        (false, false) => {}
    }
    for (left, right) in left.iter().zip(right) {
        let compared = match (left, right) {
            (Identifier::Numeric(left), Identifier::Numeric(right)) => {
                left.len().cmp(&right.len()).then_with(|| left.cmp(right))
            }
            (Identifier::Numeric(_), Identifier::Text(_)) => Ordering::Less,
            (Identifier::Text(_), Identifier::Numeric(_)) => Ordering::Greater,
            (Identifier::Text(left), Identifier::Text(right)) => left.cmp(right),
        };
        if compared != Ordering::Equal {
            return compared;
        }
    }
    left.len().cmp(&right.len())
}

/// A directive recognized on a logical line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Directive {
    Import,
    TryImport,
    ConditionalImport(VersionCondition),
}

/// What Bazel reads from a nonempty logical line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Statement {
    Entry,
    Directive(Directive),
    InvalidDirective,
}

/// One logical line, retaining its tokens and trailing comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub range: Span,
    pub tokens: Vec<Token>,
    pub comment: Option<Span>,
    pub statement: Option<Statement>,
}

impl Line {
    #[must_use]
    pub fn key(&self) -> Option<&Token> {
        self.tokens.first()
    }

    #[must_use]
    pub fn options(&self) -> &[Token] {
        self.tokens.get(1..).unwrap_or_default()
    }
}

/// One structural error Bazel would reject before interpreting flags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub range: Span,
    pub message: String,
}

/// A lossless-enough view for editor operations: every meaningful byte keeps
/// its physical range, including a token joined across a continuation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parse {
    pub lines: Vec<Line>,
    pub errors: Vec<Error>,
}

/// One `--config` value on an entry line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigReference {
    pub name: String,
    pub range: Span,
}

/// One named configuration declaration and the physical range of its name.
#[derive(Debug, Clone, Copy)]
pub struct ConfigDeclaration<'a> {
    pub command: &'a str,
    pub name: &'a str,
    pub range: Span,
}

/// A named configuration body retained by Bazel 8.7's rc reader.
#[must_use]
pub fn config_declaration(line: &Line) -> Option<ConfigDeclaration<'_>> {
    if !matches!(line.statement, Some(Statement::Entry)) {
        return None;
    }
    let key = line.key()?;
    let (command, name) = key.text.split_once(':')?;
    let range = key
        .decoded_span(command.len() + 1..key.text.len())
        .unwrap_or(key.range);
    Some(ConfigDeclaration {
        command,
        name,
        range,
    })
}

/// Named configuration references. Bazel accepts split option/value spelling
/// only outside a named configuration body.
#[must_use]
pub fn config_references(line: &Line) -> Vec<ConfigReference> {
    let mut found = Vec::new();
    let mut options = line.options().iter().peekable();
    while let Some(option) = options.next() {
        if let Some(name) = option.text.strip_prefix("--config=") {
            let range = option
                .decoded_span("--config=".len()..option.text.len())
                .unwrap_or(option.range);
            found.push(ConfigReference {
                name: name.to_owned(),
                range,
            });
        } else if !line.key().is_some_and(|key| key.text.contains(':'))
            && option.text == "--config"
            && let Some(value) = options.next()
        {
            found.push(ConfigReference {
                name: value.text.clone(),
                range: value
                    .decoded_span(0..value.text.len())
                    .unwrap_or(value.range),
            });
        }
    }
    found
}

/// Parse one Bazel 8.7 rc buffer.
#[must_use]
pub fn parse(text: &str) -> Parse {
    let logical = Logical::new(text);
    let mut lines = Vec::new();
    let mut errors = Vec::new();
    let mut start = 0;
    loop {
        let end = logical.bytes[start..]
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(logical.bytes.len(), |offset| start + offset);
        lines.push(parse_line(&logical, start, end, &mut errors));
        if end == logical.bytes.len() {
            break;
        }
        start = end + 1;
    }
    Parse { lines, errors }
}

struct Logical {
    bytes: Vec<u8>,
    /// Physical source offset at each boundary between retained bytes.
    boundaries: Vec<usize>,
}

impl Logical {
    fn new(text: &str) -> Self {
        let source = text.as_bytes();
        let mut bytes = Vec::with_capacity(source.len());
        let mut boundaries = vec![0];
        let mut at = 0;
        while at < source.len() {
            let removed = if source[at] == b'\\' && source.get(at + 1) == Some(&b'\n') {
                2
            } else if source[at] == b'\\'
                && source.get(at + 1) == Some(&b'\r')
                && source.get(at + 2) == Some(&b'\n')
            {
                3
            } else {
                0
            };
            if removed != 0 {
                at += removed;
                *boundaries.last_mut().expect("the initial boundary") = at;
                continue;
            }
            bytes.push(source[at]);
            at += 1;
            boundaries.push(at);
        }
        Self { bytes, boundaries }
    }

    fn span(&self, start: usize, end: usize) -> Span {
        Span::new(self.boundaries[start], self.boundaries[end])
    }
}

fn parse_line(logical: &Logical, start: usize, end: usize, errors: &mut Vec<Error>) -> Line {
    let (tokens, comment) = tokenize(logical, start, end);
    let statement = statement(&tokens, logical.span(start, end), errors);
    Line {
        range: logical.span(start, end),
        tokens,
        comment,
        statement,
    }
}

fn statement(tokens: &[Token], range: Span, errors: &mut Vec<Error>) -> Option<Statement> {
    let key = tokens.first()?.text.as_str();
    let expected = match key {
        "import" | "try-import" => Some(2),
        "try-import-if-bazel-version" => Some(3),
        _ => None,
    };
    if let Some(expected) = expected
        && tokens.len() != expected
    {
        errors.push(Error {
            range,
            message: format!("`{key}` expects {} argument(s)", expected - 1),
        });
        return Some(Statement::InvalidDirective);
    }
    match key {
        "import" => Some(Statement::Directive(Directive::Import)),
        "try-import" => Some(Statement::Directive(Directive::TryImport)),
        "try-import-if-bazel-version" => {
            let condition = VersionCondition::parse(&tokens[1].text);
            if let Some(condition) = condition {
                Some(Statement::Directive(Directive::ConditionalImport(
                    condition,
                )))
            } else {
                errors.push(Error {
                    range: tokens[1].range,
                    message: "invalid Bazel version condition".to_owned(),
                });
                Some(Statement::InvalidDirective)
            }
        }
        _ if tokens.len() >= 2 => Some(Statement::Entry),
        _ => None,
    }
}

fn tokenize(logical: &Logical, start: usize, end: usize) -> (Vec<Token>, Option<Span>) {
    let mut tokens = Vec::new();
    let mut at = start;
    while at < end {
        while at < end && delimiter(logical.bytes[at]) {
            at += 1;
        }
        if at == end {
            break;
        }
        if logical.bytes[at] == b'#' {
            return (tokens, Some(logical.span(at, end)));
        }

        let token_start = at;
        let mut value = Vec::new();
        let mut origins = Vec::new();
        let mut quote = None;
        let mut has_value = false;
        while at < end {
            let byte = logical.bytes[at];
            if let Some(open) = quote {
                if byte == open {
                    quote = None;
                    at += 1;
                } else if byte == b'\\' {
                    let origin = at;
                    at += 1;
                    if at < end {
                        value.push(logical.bytes[at]);
                        has_value = true;
                        at += 1;
                        origins.push(logical.span(origin, at));
                    }
                } else {
                    value.push(byte);
                    has_value = true;
                    origins.push(logical.span(at, at + 1));
                    at += 1;
                }
            } else if delimiter(byte) {
                break;
            } else if byte == b'#' {
                if has_value {
                    tokens.push(token(value, origins, logical.span(token_start, at)));
                }
                return (tokens, Some(logical.span(at, end)));
            } else if matches!(byte, b'\'' | b'"') {
                quote = Some(byte);
                at += 1;
            } else if byte == b'\\' {
                let origin = at;
                at += 1;
                if at < end {
                    value.push(logical.bytes[at]);
                    has_value = true;
                    at += 1;
                    origins.push(logical.span(origin, at));
                }
            } else {
                value.push(byte);
                has_value = true;
                origins.push(logical.span(at, at + 1));
                at += 1;
            }
        }
        if has_value {
            tokens.push(token(value, origins, logical.span(token_start, at)));
        }
    }
    (tokens, None)
}

fn token(value: Vec<u8>, origins: Vec<Span>, range: Span) -> Token {
    Token {
        text: String::from_utf8(value).expect("removing ASCII syntax preserves UTF-8"),
        range,
        origins,
    }
}

const fn delimiter(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\r' | b'\n')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continuation_is_deleted_before_tokenization() {
        let text = "build --define=one=\\\n two # tail\n";
        let parsed = parse(text);
        assert!(parsed.errors.is_empty());
        let line = &parsed.lines[0];
        assert_eq!(
            line.tokens
                .iter()
                .map(|token| token.text.as_str())
                .collect::<Vec<_>>(),
            ["build", "--define=one=", "two"]
        );
        assert_eq!(line.tokens[1].range, Span::new(6, 21));
        assert_eq!(line.comment, Some(Span::new(26, 32)));
    }

    #[test]
    fn comments_quotes_and_escapes_match_the_native_tokenizer() {
        let parsed = parse("build foo' bar'\"#baz\" qux\\ quux end#comment\n");
        let line = &parsed.lines[0];
        assert_eq!(
            line.tokens
                .iter()
                .map(|token| token.text.as_str())
                .collect::<Vec<_>>(),
            ["build", "foo bar#baz", "qux quux", "end"]
        );
        assert_eq!(
            &parse("common '' --define=x=1\n").lines[0]
                .tokens
                .iter()
                .map(|token| token.text.as_str())
                .collect::<Vec<_>>(),
            &["common", "--define=x=1"]
        );
    }

    #[test]
    fn unfinished_quotes_and_escapes_are_not_syntax_errors() {
        for text in ["build '--define=x=1\n", "build --define=x=1\\\n"] {
            assert!(parse(text).errors.is_empty(), "{text:?}");
        }
    }

    #[test]
    fn directives_have_exact_arity_and_conditions() {
        let parsed =
            parse("import one two\ntry-import maybe\ntry-import-if-bazel-version >=8.7.0 path\n");
        assert_eq!(parsed.errors.len(), 1);
        assert_eq!(
            parsed.lines[1].statement,
            Some(Statement::Directive(Directive::TryImport))
        );
        assert!(matches!(
            parsed.lines[2].statement,
            Some(Statement::Directive(Directive::ConditionalImport(_)))
        ));
    }

    #[test]
    fn an_argless_entry_does_not_declare_an_empty_config() {
        let parsed = parse("common:empty\n");
        assert_eq!(parsed.lines[0].statement, None);
        assert!(config_declaration(&parsed.lines[0]).is_none());

        let text = "common:present --define=x=1\n";
        let parsed = parse(text);
        let declaration = config_declaration(&parsed.lines[0]).unwrap();
        assert_eq!(
            (declaration.command, declaration.name),
            ("common", "present")
        );
        assert_eq!(
            &text[declaration.range.start..declaration.range.end],
            "present"
        );
    }

    #[test]
    fn config_names_map_decoded_bytes_to_exact_physical_spans() {
        let text = "'build:de\\v' --config=\"ch\\ild\"\n";
        let parsed = parse(text);
        let declaration = config_declaration(&parsed.lines[0]).unwrap();
        assert_eq!(declaration.name, "dev");
        assert_eq!(
            &text[declaration.range.start..declaration.range.end],
            "de\\v"
        );
        let reference = &config_references(&parsed.lines[0])[0];
        assert_eq!(reference.name, "child");
        assert_eq!(&text[reference.range.start..reference.range.end], "ch\\ild");
    }

    #[test]
    fn split_config_references_are_only_top_level() {
        let parsed = parse("build --config dev\nbuild:outer --config inner\n");
        assert_eq!(config_references(&parsed.lines[0])[0].name, "dev");
        assert!(config_references(&parsed.lines[1]).is_empty());
    }

    #[test]
    fn conditions_follow_semver_precedence() {
        assert!(
            !VersionCondition::parse(">=8.7.0")
                .unwrap()
                .matches("8.7.0-imc2")
        );
        assert!(
            VersionCondition::parse(">=8.7.0-imc2")
                .unwrap()
                .matches("8.7.0-imc2")
        );
        assert!(VersionCondition::parse("~8").unwrap().matches("8.99.0"));
        assert!(!VersionCondition::parse("~8.2").unwrap().matches("8.3.0"));
        assert!(VersionCondition::parse("~8.2.4").unwrap().matches("8.2.99"));
        assert!(
            VersionCondition::parse("==8.7.0+one")
                .unwrap()
                .matches("8.7.0+two")
        );
        assert!(VersionCondition::parse(">=8.7").is_none());
        assert!(VersionCondition::parse("~08").is_none());
    }

    #[test]
    fn crlf_continuation_has_one_logical_line() {
        let text = "build --define=x=\\\r\ny\r\n";
        let parsed = parse(text);
        assert_eq!(parsed.lines[0].options()[0].text, "--define=x=y");
        assert_eq!(parsed.lines[0].options()[0].range, Span::new(6, 21));
    }
}
