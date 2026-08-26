# 07 — Parsing and incremental-analysis infrastructure

Date of assessment: **2026-08-25**. Everything below was checked against the local
`upstream/` clones and, where stated, executed.

Tool versions actually used: `buildifier 8.5.1` (`scm revision: v8.5.1`, from
`nix shell nixpkgs#bazel-buildtools`), `tree-sitter 0.26.9` CLI, `nix 2.34.8`.

---

## 0. Liveness summary (`git log -1` on the local clones)

| Project | HEAD | Date | Verdict |
|---|---|---|---|
| `bazelbuild/buildtools` | `674b2934` | **2026-08-24** | Alive, daily |
| `facebook/starlark-rust` | `79bc31f3` | **2026-08-24** | Alive, daily (Buck2 depends on it) |
| `bazelbuild/bazel` | `3a9b19c8` | **2026-08-24** | Alive |
| `withered-magic/starpls` | `ac25eca3` | 2025-12-03 | Alive but slow; 5-month gap Dec 2024→Jun 2025 |
| `google/starlark-go` | `5395d018` | 2026-07-08 | Alive, low traffic |
| `tree-sitter-grammars/tree-sitter-starlark` | `a453dbf3` | **2024-12-04** | Stale 20 months; last GH push 2025-05-26 |
| `cameron-martin/bazel-lsp` | `48fead62` | 2025-07-20 | Stale 13 months (last commit is a renovate `cc` bump) |
| `tilt-dev/starlark-lsp` | `5689e7e8` | 2024-07-30 | **Abandoned** (last commit is a CircleCI golang bump) |

---

## 1. tree-sitter-starlark

`upstream/tree-sitter-starlark`, MIT, author Amaan Qureshi (`amaanq`). Repo moved to
the community `tree-sitter-grammars` org; that org is a grammar dumping ground, not a
maintained product line — this grammar has 24 stars, 3 forks, and 2 open bugs.

### It is tree-sitter-python with 15 rules overridden

`grammar.js` is **215 lines**:

```js
const Python = require('tree-sitter-python/grammar');

module.exports = grammar(Python, {
  name: 'starlark',
  conflicts: (_, original) => original.filter((e) =>
    !(e.length == 2 && e[0].name == 'match_statement' && e[1].name == 'primary_expression'),
  ),
  rules: { /* 15 overridden rules */ },
});
```

Everything not overridden is inherited from Python. The consequences are concrete:

- `_compound_statement` still lists `$.while_statement`, `$.with_statement`,
  `$.match_statement`, `$.decorated_definition`. The grammar comment says
  *"Google's implementation of Starlark supports while statements"* — this is **false
  for Bazel**. `Parser.java` puts `WHILE` and `WITH` in `FORBIDDEN_KEYWORDS` and emits
  `'while' not supported, use 'for' instead`.
- `_simple_statement` keeps `$.print_statement`, `$.exec_statement`,
  `$.assert_statement` (with a whole Starlark-*test*-flavoured `assert.eq`/`assert_ne`
  rule). None are Bazel Starlark.
- Inherited: `set`, `set_comprehension`, `ellipsis`, f-strings, Python type parameters.
- `keyword_identifier` aliases `print`/`exec`/`async`/`await`/`match`/`struct`/`type` to
  plain identifiers.

It **over-accepts** by a wide margin. For an LSP that is not fatal (you can validate
after the fact) but it means the grammar cannot be the source of truth for diagnostics.

### It has zero BUILD-file modelling

There is no `load` node. `test/corpus/load.txt` is the proof — a `load()` statement
parses as:

```
(module (expression_statement (call (identifier) (argument_list (string …) (string …) (string …)))))
```

No `LoadStmt`, no module/symbol distinction, no aliased-load node. Same for `glob`,
`select`, `package`, `exports_files`. Every consumer re-derives all of it by matching on
`(call)` and string-comparing the `function` field.

`tree-sitter.json` declares `"file-types": ["bzl"]` and `"scope": "source.bzl"` only —
`BUILD`, `BUILD.bazel`, `WORKSPACE`, `MODULE.bazel` are not mapped; the editor must map
them. It also declares `"tags": "queries/tags.scm"`, and **`queries/tags.scm` does not
exist** (`queries/` contains only `folds/highlights/indents/injections/locals.scm`). So
no symbol-extraction query ships.

### Error recovery: genuinely excellent (measured)

I wrote `/tmp/tsdemo/BUILD.partial` with an unterminated string mid-`deps`:

```
cc_library(
    name = "foo",
    srcs = ["a.cc"],
    deps = [
        ":ba          ← unterminated
    ],
)

cc_binary(name = "bar", deps = ["//x:y"])
```

`tree-sitter parse` output (abridged):

```
        (keyword_argument [5, 4] - [7, 5]
          name: (identifier [5, 4] - [5, 8])
          value: (list [5, 11] - [7, 5]
            (ERROR [6, 8] - [6, 10]
              (string_start [6, 8] - [6, 9]))
            (identifier [6, 10] - [6, 12]))))))
  (expression_statement [10, 0] - [13, 1]
    (call [10, 0] - [13, 1] …          ← cc_binary parses cleanly
```

The ERROR node is **two characters wide**, stays inside the `deps` list, and the rest of
the file is unaffected. This is best-in-class recovery and is the single strongest
argument for tree-sitter.

`test/corpus/errors.txt` shows the same shape for a dangling `c.` inside a `def`.

### Throughput (measured on `upstream/bazel`)

```
107 .bzl files   : 100% parsed, average speed 19,698 bytes/ms
575 BUILD files  : 100% parsed, average speed 32,084 bytes/ms   (--scope source.bzl)
```

≈ 20–32 MB/s single-threaded.

### Losslessness, incrementality

Lossless in the tree-sitter sense: the CST covers every byte, so source is recoverable
by slicing. Comments are `(comment)` **siblings**, not attached to the node they
document — reattachment is the consumer's problem. Whitespace is the implicit gap
between node byte ranges, not a node.

Incremental: `ts_tree_edit` + reparse is the headline feature and it works. Rust
bindings expose it.

### Rust bindings on crates.io: stale

```
tree-sitter-starlark  1.3.0   published 2024-12-05   177,184 downloads
  dependencies:      tree-sitter-language = "0.1"
  dev-dependencies:  tree-sitter = "0.24"
```

Current `tree-sitter` on crates.io is **0.26.13 (2026-08-23)**. The checked-in
`src/parser.c` has `#define LANGUAGE_VERSION 14`, `STATE_COUNT 2260`,
`SYMBOL_COUNT 244`, 96,925 lines, generated with `tree-sitter-cli ^0.24.4`. ABI 14 is
still loadable by 0.26 (the `tree-sitter-language 0.1` shim exists for exactly this), so
it will link — it is just 20 months unmaintained.

### Open bugs that matter

- **#9, opened 2025-10-11, still open**: *"bug: starlark's grammar is incompatible with
  recent python's grammar"*. This is a direct structural consequence of
  `require('tree-sitter-python/grammar')`: every upstream Python grammar release can
  break Starlark, and there is nobody regenerating.
- **#7, opened 2025-02-03 by `keith` (Bazel/Apple ecosystem), still open**:
  *"`keyword_argument` in BUILD files doesn't contain trailing comma"*. A losslessness
  defect, and precisely the kind that breaks a formatter.

### How tilt-dev/starlark-lsp uses it — it doesn't

**It uses tree-sitter-python, not tree-sitter-starlark.**

```go
// pkg/query/lang.go
var LanguagePython = python.GetLanguage()

// pkg/query/query.go:23
q := MustQuery([]byte(pattern), LanguagePython)
```

`go.mod` pins `github.com/smacker/go-tree-sitter v0.0.0-20220209044044-0d3022e933c3`
(2022-02-09) and uses its bundled Python grammar.

Queries are **Go string constants, not `.scm` files** — there are no `.scm` files
anywhere in the repo:

| Constant | File | Purpose |
|---|---|---|
| `Identifiers` | `pkg/query/identifiers.go:10` | completion-context identifier chain |
| `FunctionParameters` | `pkg/query/params.go:21` | signature help / param docs |
| `structs` | `pkg/query/structs.go:8` | `x = struct(a=…, b=…)` → symbol |
| `methodsAndFields` | `pkg/query/types.go:9` | attribute completion |
| `(call) @call` | `pkg/query/queries.go:26` | `LoadStatements` — filters `function` text == `"load"` |

Two techniques worth stealing regardless of front end:

1. **Load detection is a post-filter on `(call)`**, because the grammar has no load node:
   ```go
   Query(tree.RootNode(), `(call) @call`, func(...) bool {
       id := c.Node.ChildByFieldName("function")
       if doc.Content(id) == "load" { nodes = append(nodes, c.Node) }
   })
   ```
2. **ERROR nodes are deliberately queried** to drive completion after a dangling dot:
   ```
   [(module) @module
    (identifier) @id
    "." @dot
    (ERROR "." @trailing-dot .)]
   ```
   That pattern — treat the parser's error node as a completion trigger — is the right
   idea in any front end.

Document symbols (`pkg/query/symbol.go`) are hand-walked (`SiblingSymbols` switches on
`NodeTypeExpressionStatement` / `NodeTypeFunctionDef`), not query-driven.

---

## 2. starpls's own syntax crates

`upstream/starpls`, dual **Apache-2.0 / MIT**. This is the language server
`bazelbuild/vscode-bazel` documents as `"bazel.lsp.command": "starpls"`.

### Layout is a 1:1 copy of rust-analyzer

```
crates/starpls_lexer     2,336 LOC   (hand-written cursor lexer, rustc_lexer-derived)
crates/starpls_parser    2,269 LOC   (event-based recursive descent + SyntaxKind)
crates/starpls_syntax    1,730 LOC   (rowan glue + generated AST)
crates/starpls_common      228 LOC   (salsa base db: File input, parse query)
crates/starpls_hir      15,249 LOC   (scopes, codeflow, lower, typeck, builtins)
crates/starpls_ide       3,709 LOC   (features)
crates/starpls_bazel / _intern / starpls (server)
```

### Parser: hand-written recursive descent, rust-analyzer's exact design

`crates/starpls_parser/src/lib.rs`:

```rust
pub(crate) struct Parser<'a> {
    input: &'a Input,          // Vec<SyntaxKind>, trivia already filtered out
    events: Vec<StepEvent>,    // Tombstone | Token | Error
    pos: usize,
}
pub fn parse(input: &Input) -> Output {
    let mut p = Parser::new(input);
    grammar::module(&mut p);
    step::postprocess_step_events(p.events)
}
```

Doc comment states the invariant explicitly: *"Because the parser operates only on token
types and has no knowledge of text offsets, etc., it instead outputs a series of steps
that can be consumed by a separate parse tree builder."* Same `Marker`/`complete()`
protocol as ra (`marker.rs`, 61 LOC).

Trivia is re-attached by `StrWithTokens::build_with_trivia` (`text.rs:95`), which drives
a `&mut dyn FnMut(StrStep)` sink with a 3-state machine
(`Init`/`Normal`/`PendingFinish`) so comments/whitespace land inside the node that
follows them.

### It is rowan-based, and parsing never fails

`crates/starpls_syntax/src/parser.rs`:

```rust
pub fn parse_module(input: &str, errors_sink: &mut dyn FnMut(SyntaxError)) -> ParseTree<Module>
```

No `Result`. Errors go out a sink; the function asserts the root green node is `MODULE`.
`rowan = "0.15.11"` (crates.io current is **0.17.0**, 2026-08-02 — two minors behind).

Neat trick: `# type: ` comments are re-lexed and parsed into a **nested subtree inside
the green tree** (`build_type_comment`, parser.rs:97) using a second grammar entry point
`parse_type_list`. So a COMMENT token is sometimes a `TYPE_COMMENT` node containing
`TYPE_COMMENT_PREFIX` + a parsed type expression.

### `SyntaxKind`

One flat `#[repr(u16)]` enum, **175 variants**, `crates/starpls_parser/src/syntax_kind.rs`:

- **Tokens** (indices 0–100): `ERROR, EOF, COMMENT, NEWLINE, WHITESPACE, INDENT, DEDENT,
  IDENT, INT, FLOAT, STRING, BYTES`, keywords (including the *reserved-but-illegal* ones
  `AS ASSERT ASYNC AWAIT CLASS DEL EXCEPT FINALLY FROM GLOBAL IGNORE IMPORT IS NONLOCAL
  RAISE TRY WHILE WITH YIELD` — lexed so the parser can produce a good error), all
  operators, `ARROW`, `ELLIPSIS`.
- **Expressions**: `NAME, NAME_REF, LITERAL_EXPR, IF_EXPR, UNARY_EXPR, BINARY_EXPR,
  LAMBDA_EXPR, LIST_EXPR, LIST_COMP, DICT_EXPR, DICT_COMP, TUPLE_EXPR, PAREN_EXPR,
  DOT_EXPR, CALL_EXPR, INDEX_EXPR, SLICE_EXPR`.
- **Statements**: `DEF_STMT, IF_STMT, FOR_STMT, RETURN_STMT, BREAK_STMT, CONTINUE_STMT,
  PASS_STMT, ASSIGN_STMT, LOAD_STMT`.
- **A whole type sub-language** for `# type:` comments: `NONE_TYPE, UNION_TYPE,
  ELLIPSIS_TYPE, PATH_TYPE, GENERIC_ARGUMENTS, PATH_SEGMENT, IGNORE_TYPE, FUNCTION_TYPE,
  PARAMETER_TYPES, SIMPLE_PARAMETER_TYPE, ARGS_LIST_PARAMETER_TYPE,
  KWARGS_DICT_PARAMETER_TYPE, TYPE_COMMENT, TYPE_COMMENT_PREFIX, TYPE_COMMENT_BODY,
  TYPE_LIST`.
- **Calls/params**: `ARGUMENTS, SIMPLE_ARGUMENT, KEYWORD_ARGUMENT,
  UNPACKED_LIST_ARGUMENT, UNPACKED_DICT_ARGUMENT, PARAMETERS, SIMPLE_PARAMETER,
  ARGS_LIST_PARAMETER, KWARGS_DICT_PARAMETER`.
- **Structure**: `SUITE, LOOP_VARIABLES, COMP_CLAUSE_FOR, COMP_CLAUSE_IF, DICT_ENTRY`.
- **`load` is first class**: `LOAD_STMT, LOAD_MODULE, DIRECT_LOAD_ITEM,
  ALIASED_LOAD_ITEM`. This is exactly what tree-sitter-starlark lacks.
- `MODULE` is the root.

`SyntaxKindSet(u128)` is a bitset over token kinds. `ELLIPSIS` is index 100 — **27 slots
of headroom before the u128 silently overflows** (`1 << kinds[i]` with no bounds check).
A latent bug for anyone extending it.

### Error recovery: correct in shape, crude in tuning

```rust
pub(crate) fn error_recover_until(&mut self, message: impl Into<String>, recover: SyntaxKindSet) {
    self.error(message);
    if !self.at(EOF) && !recover.contains(self.current()) {
        let m = self.start();
        while !self.at(EOF) && !recover.contains(self.current()) { self.bump_any(); }
        m.complete(self, ERROR);
    }
}
```

There is exactly **one** recovery set in the whole parser:

```rust
// grammar/statements.rs:13
pub(crate) const STMT_RECOVERY: SyntaxKindSet = SyntaxKindSet::new(&[T!['\n']]);
```

and it is used at all 36 recovery sites, including inside argument lists
(`"(" was not closed`), lists (`"[" was not closed`), and dicts. So a stray token inside
`deps = [...]` skips to the next newline rather than to the next `,`/`]`. Messages are
decent and human (`Expected function name`, `"(" was not closed`, `Expected statement
suite`, `Expected loop variables`).

Practical blast radius is still one line, because the lexer refuses to run a
non-triple-quoted string past a newline (`fn string`, `starpls_lexer/src/lib.rs`:
`if (self.first() == '\n' …) && !triple_quoted { return (false, triple_quoted); }`).

Two defects to fix in any fork:

1. Parser errors get **zero-width ranges**:
   ```rust
   errors_sink(SyntaxError { message, range: TextRange::new(TextSize::new(token_pos), TextSize::new(token_pos)) });
   ```
   Editors render a 0-character squiggle. (Lexer errors *do* get real ranges.)
2. No per-context recovery sets, as above.

### Incrementality

Per-file salsa memoization only. `parse` is a `#[salsa::tracked]` query keyed on the
`File` input; any keystroke re-parses the whole file from scratch. No `rowan`
green-node reuse, no incremental relex. Given tree-sitter parses at 20–32 MB/s and BUILD
files average ~4 KB, full reparse is ~0.2 ms — this is the right trade.

### LSP surface actually implemented (`event_loop.rs:214-220`)

`Completion`, `DocumentSymbolRequest`, `GotoDefinition`, `GotoDeclaration`, `Hover`,
`References`, `SignatureHelp`. That is all. **No formatting, no workspace symbols, no
rename, no code actions, no semantic tokens.** `find_references` is single-file — it
`memchr`s the identifier inside `self.file.contents(db)` and never leaves the file
(`crates/starpls_ide/src/find_references.rs:28-35`).

---

## 3. facebook/starlark-rust

`upstream/starlark-rust`, Apache-2.0, HEAD 2026-08-24, `starlark_syntax` **0.14.2**,
Rust edition 2024. Meta ships it inside Buck2, so it is genuinely well-maintained.

### LALRPOP is gone

There are **no `.lalrpop` files in the tree**. `starlark_syntax/src/syntax.rs` declares
only `pub(crate) mod parser_rd;`. `starlark_syntax/src/syntax/parser_lalrpop.rs`
(138 LOC) is an **orphaned file not in the module tree** — it imports
`crate::syntax::grammar::StarlarkParser`, `crate::syntax::parse_error::ParseError` and
`crate::syntax::parser::Parser`, none of which exist.

`CHANGELOG.md`, 0.14 (2026-05-20):

> - Add a hand-written recursive-descent + Pratt expression parser as an alternative to
>   LALRPOP, roughly 2x faster on per-parse microbenchmarks. Selectable via `ParserKind`.
> - Expose comment spans from the parser via `AstModule::comments()`.

By HEAD the LALRPOP arm has been deleted and `ParserKind` with it. Lexer is `logos 0.15`.

`parser_rd.rs` (1,487 LOC) doc header: *"Recursive descent + Pratt expression parser…
Production names mirror the non-terminals in the spec's grammar… The Pratt parsing
algorithm follows matklad's 'Simple but Powerful Pratt Parsing'."*

### The AST is evaluator-first, and here is what that costs

`starlark_syntax/src/syntax/ast.rs` (837 LOC). Everything is `Spanned<T>` over
`Span{Pos(u32), Pos(u32)}`, and every node type is generic over an `AstPayload`:

```rust
pub trait AstPayload: Debug {
    type LoadPayload;  type IdentPayload;  type IdentAssignPayload;
    type DefPayload;   type TypeExprPayload;
}
pub enum StmtP<P: AstPayload> {
    Break, Continue, Pass, Return(Option<AstExprP<P>>), Expression(AstExprP<P>),
    Assign(AssignP<P>), AssignModify(…), Statements(Vec<AstStmtP<P>>),
    If(…), IfElse(…), For(ForP<P>), Def(DefP<P>), Load(LoadP<P>),
}
```

The payload slot is precisely so the resolver can staple `Slot`/`BindingId` onto
identifiers before compilation. Excellent for an interpreter. For an LSP:

**Cost 1 — comments are thrown away.** `AstModule::parse` (`module.rs:157-183`) filters
them out of the lexeme stream before the parser sees them:

```rust
let filtered = lexer.filter(|token| match token {
    Ok((start, Token::Comment(comment), end)) => {
        lint_suppressions_builder.parse_comment(&codemap, comment, *start, *end);
        comment_spans.push(Span::new(Pos::new(*start as u32), Pos::new(*end as u32)));
        in_comment_block = true;
        false                     // ← dropped
    }
    _ => { … true }
});
```

You get a flat `Vec<Span>` via `AstModule::comments()`. Never attached to a node.
Whitespace is gone entirely. So it cannot back a formatter, cannot back
`# buildifier: leave-alone` / `# do not sort` handling, and cannot back doc-comment
hover without a second pass over the source text.

**Cost 2 — the tree is desugared.** `elif` chains collapse into nested `IfElse`;
`Stmt::Statements` flattens blocks; `LoadP` synthesises fake local `Ident`s for
non-aliased symbols (buildtools documents the same fudge). Round-tripping is impossible.

**Cost 3 — no error recovery. This is disqualifying.**

```rust
match parser_rd::parse_module(&mut state, filtered) {
    Ok(v) => {
        if let Some(err) = errors.into_iter().next() {
            return Err(err.into_error());          // ← first error wins, tree discarded
        }
        Ok(AstModule::create(…))
    }
    Err(e) => Err(e.into_error()),
}
```

The proof of what this costs is in Meta's own LSP,
`starlark_lsp/src/server.rs`:

```rust
/// The `AstModule` from the last time that a file was opened / changed and parsed successfully.
pub(crate) last_valid_parse: RwLock<HashMap<LspUri, Arc<LspModule>>>,
…
/// NOTE: This uses the last valid parse of a file as a basis for symbol locations.
/// If a file has changed and does result in a valid parse, then symbol locations may
/// [be stale]
```

While you are mid-keystroke — which is *always*, for completion — the server answers
from a stale snapshot. That is the single most user-visible defect an LSP can have.

`Dialect` (`dialect.rs:36`) is a good idea worth copying: `enable_def`, `enable_lambda`,
`enable_load`, `enable_keyword_only_arguments`, `enable_positional_only_arguments`,
`enable_types: DialectTypes`, `enable_load_reexport`, `enable_top_level_stmt`,
`enable_f_strings`. It is *not* Bazel's dialect matrix (no BUILD-vs-bzl-vs-MODULE
distinction, no `select`, no label type), but the shape is right.

`bazel-lsp` (`upstream/bazel-lsp`) consumes it via git deps on
`facebook/starlark-rust` `main` for `starlark`, `starlark_lsp`, `starlark_syntax` — and
is itself 13 months stale.

---

## 4. bazelbuild/buildtools — the one that actually matters

`upstream/buildtools`, Apache-2.0, HEAD `674b2934` **2026-08-24**, latest tag `v8.5.1`,
which is what nixpkgs ships. `build/` is 9,224 LOC of Go.

### Parser: goyacc, and it has no error recovery

`build/parse.y` is a yacc grammar (`%union` carrying `tok/str/pos/triple/expr/exprs/kv/
kvs/string/ifstmt/loadarg/loadargs/def_header/comma/lastStmt`) generated into
`build/parse.y.go`; `build/lex.go` is the hand-written lexer. `parse.y` contains **no
`error` productions**.

```go
// build/lex.go:305
// Error does not return: it panics.
func (in *input) Error(s string) {
	if s == "syntax error" && in.lastToken != "" {
		s += " near " + in.lastToken
	}
	in.parseError = ParseError{Message: s, Filename: in.filename, Pos: in.pos}
	panic(in.parseError)
}
```

`ParseBuild`/`ParseBzl`/… `defer` a `recover()` and convert the panic into a single
error. Demonstrated:

```
$ buildifier --format=json --mode=check --lint=warn BUILD.broken
BUILD.broken:3:9: syntax error near srcs
{"success":false,"files":[{"filename":"BUILD.broken","formatted":false,"valid":false,"warnings":[]}]}
```

One error, no partial tree, no warnings. **This is why buildifier cannot be the LSP's
parser** — but it can still be the LSP's formatter/linter, invoked on a
known-syntactically-valid buffer.

### File types

```go
// build/lex.go:34
type FileType int
const (
	TypeDefault FileType = 1 << iota   // generic Starlark
	TypeBuild                          // BUILD, BUILD.bazel
	TypeWorkspace                      // WORKSPACE, WORKSPACE.bazel
	TypeBzl                            // *.bzl
	TypeModule                         // MODULE.bazel, *.MODULE.bazel
	TypeRepo                           // REPO.bazel
	TypeVendor                         // vendor manifests
)
```

Entry points `ParseBuild / ParseWorkspace / ParseModule / ParseRepo / ParseVendor /
ParseBzl / ParseDefault`, dispatched by `Parse(filename, data)` on the basename.

### AST node types (`build/syntax.go`, 838 LOC, 35 structs)

```go
type Position struct { Line int; LineRune int; Byte int }   // Line/LineRune 1-based, Byte 0-based

type Expr interface {
	Span() (start, end Position)   // excludes leading/trailing comments
	Comment() *Comments
	Copy() Expr
}

type Comment  struct { Start Position; Token string }        // no trailing newline
type Comments struct { Before []Comment; Suffix []Comment; After []Comment }
```

Every node embeds `Comments`. Full list:

| Node | Notable fields |
|---|---|
| `File` | `Path, Pkg, Label, WorkspaceRoot, Type FileType, Comments, Stmt []Expr` |
| `CommentBlock` | top-level comment island |
| `Ident` | `NamePos, Name` |
| `TypedIdent` | `Ident, Type Expr` |
| `BranchStmt` | `pass`/`break`/`continue` |
| `LiteralExpr` | numeric literal, raw `Token` kept |
| `StringExpr` | `Start, Value (decoded), TripleQuote bool, End, Token (original quoted form, a hint)` |
| `End` | *"the end of a parenthesized or bracketed expression. It is a place to hang comments."* |
| `CallExpr` | `X, ListStart, List []Expr, End, ForceCompact, ForceMultiLine` |
| `DotExpr` | `X, Dot, NamePos, Name` |
| `Comprehension`, `ForClause`, `IfClause` | |
| `KeyValueExpr`, `DictExpr`, `ListExpr`, `SetExpr`, `TupleExpr` | |
| `UnaryExpr`, `BinaryExpr`, `AssignExpr`, `ParenExpr`, `SliceExpr`, `IndexExpr` | |
| `Function`, `LambdaExpr`, `ConditionalExpr` | |
| `LoadStmt` | `Load Position, Module *StringExpr, From []*Ident, To []*Ident, Rparen End, ForceCompact` |
| `DefStmt` | `Function, Name, ColonPos, ForceCompact, ForceMultiLine, Type Expr` |
| `ReturnStmt`, `ForStmt`, `IfStmt` | `IfStmt.ElsePos End`, elif chains are nested `IfStmt`s |

Losslessness mechanism, explicitly: comments on every node in three buckets, `End` nodes
that exist solely to host pre-`)` comments, `StringExpr.Token`/`TripleQuote` to preserve
quoting, `ForceCompact`/`ForceMultiLine` to preserve layout intent. Whitespace is *not*
retained — it is regenerated. That is correct for a formatter and wrong for a
source-preserving CST; do not confuse the two.

### The formatting algorithm — and why it cannot be reimplemented from a position-free AST

`Format(f)` = `Rewrite(f)` then `FormatWithoutRewriting(f)` (`build/print.go:43`).

```go
const (
	nestedIndentation = 4   // nested blocks
	listIndentation   = 4   // multiline expressions
	defIndentation    = 8   // multiline function definitions
)
type printer struct {
	fileType FileType; bytes.Buffer
	comment []Comment   // pending end-of-line comments
	margin  int         // left margin in spaces
	depth   int         // nesting inside ( ) [ ] { }
	level   int         // nesting of def/if/for blocks
	needsNewLine bool
}
```

`formattingMode()` collapses seven file types to two:
`TypeBuild|TypeWorkspace|TypeModule|TypeRepo|TypeVendor → TypeBuild`, else `TypeDefault`.

**There is no line-length limit anywhere in the printer.** The compact-vs-multiline
decision is `useCompactMode` (`print.go:963`):

1. Any element with `Comment().Before`, or a `Suffix` outside `modeDef`, or
   `end.Before` non-empty → **multiline**.
2. `modeSeq` (implicit tuple) → compact.
3. `modeCall` with zero args and `!forceMultiLine` → compact.
4. **If `p.level != 0` (nested inside def/if/for) OR `formattingMode() == TypeDefault`
   OR `mode == modeDef`, and `mode != modeLoad`: preserve the original layout.**
   ```go
   previousEnd := start
   isNewSeq := start.Line == 0
   for _, x := range *list {
       start, end := x.Span()
       isNewSeq = isNewSeq && start.Line == 0
       if isDifferentLines(&start, previousEnd) { return false }   // ← original line numbers
       if end.Line != 0 { previousEnd = &end }
   }
   …
   if !isNewSeq { return true }
   return !forceMultiLine        // wholly synthetic sequences
   ```
   `Line == 0` means "synthetic, not from the source".
5. Otherwise (top level of a BUILD-mode file): `forceMultiLine` → multiline,
   `forceCompact` → compact.

**Buildifier's output is a function of (AST, original source line numbers, file type).**
Proved:

```
$ cat t.bzl
x = foo(a = 1, b = 2)

y = foo(
    a = 1,
    b = 2,
)

$ buildifier -type=bzl   --mode=print_if_changed < t.bzl
                                    ← no output: both forms left exactly as written

$ buildifier -type=build --mode=print_if_changed < t.bzl
x = foo(
    a = 1,
    b = 2,
)

y = foo(
    a = 1,
    b = 2,
)
```

A Wadler/Prettier-style pretty printer over a position-free AST **cannot** reproduce
this. Any Rust reimplementation must carry `Position{Line, LineRune, Byte}` on every
node and preserve `ForceCompact`/`ForceMultiLine`.

Multi-line emission (`seq`, print.go:1034): `margin += listIndentation` (4), or
`defIndentation` (8) in `modeDef`; one element per line; trailing comma per
`needsTrailingComma(mode, x)` (never in `modeDef`); `end.Before` comments flushed before
the closing bracket; in `modeDef` the `)` stays on the element's line.

### The rewrites — "formatting" in Bazel means "format **and** rewrite"

`build/rewrite.go:119`, applied in this order, all with `--lint=off`:

```go
scopeDefault = TypeDefault | TypeBzl
scopeBuild   = TypeBuild | TypeWorkspace | TypeModule | TypeRepo | TypeVendor
scopeBoth    = scopeDefault | scopeBuild

{"removeParens",           removeParens,            scopeBuild},
{"callsort",               sortCallArgs,            scopeBuild},
{"label",                  fixLabels,               scopeBuild},
{"listsort",               sortStringLists,         scopeBoth},
{"multiplus",              fixMultilinePlus,        scopeBuild},
{"loadTop",                moveLoadOnTop,           scopeBoth},
{"sameOriginLoad",         compressSameOriginLoads, scopeBoth},
{"sortLoadStatements",     sortLoadStatements,      scopeBoth},
{"loadsort",               sortAllLoadArgs,         scopeBoth},
{"useRepoPositionalsSort", sortUseRepoPositionals,  TypeModule},
{"formatdocstrings",       formatDocstrings,        scopeBoth},
{"reorderarguments",       reorderArguments,        scopeBoth},
{"editoctal",              editOctals,              scopeBoth},
{"editfloat",              editFloats,              scopeBoth},
{"collapseEmpty",          collapseEmpty,           scopeBoth},
```

Demonstrated with `--lint=off`:

```
$ cat t2.bzl
cc_library(name = "a")
load(":x.bzl", "z", "y")
load(":x.bzl", "w")
cc_library(
    srcs = ["s.cc"],
    name = "b",
    visibility = ["//visibility:public"],
    deps = ["//z:z", "//a"],
)

$ buildifier -type=build --mode=print_if_changed --lint=off < t2.bzl
load(":x.bzl", "w", "y", "z")

cc_library(name = "a")

cc_library(
    name = "b",
    srcs = ["s.cc"],
    visibility = ["//visibility:public"],
    deps = [
        "//a",
        "//z",
    ],
)
```

Two `load`s merged, sorted, moved above everything, blank line inserted; `name` hoisted
first; `//z:z` canonicalised to `//z`; `deps` sorted and exploded.

Key sub-algorithms to match:

- **`fixLabels`** — joins `"//x" + ":y"` → `"//x:y"`; strips redundant target qualifiers
  (`//third_party/m4:m4` → `//third_party/m4`, `@foo//:foo` → `@foo`). Also
  `tables.StripLabelLeadingSlashes` / `ShortenAbsoluteLabelsToRelative`.
- **`sortStringLists`** — only for arguments named in `tables.IsSortableListArg`, a fixed
  ~40-entry table (`deps, srcs, hdrs, data, tags, outs, exports, includes, imports,
  packages, resources, runtime_deps, default_visibility, implementation_deps,
  private_deps, proto_deps, …`), modulated by `SortableDenylist` / `SortableAllowlist`,
  all overridable with `-tables` / `-add_tables`.
  Sort key (`makeSortKey`, rewrite.go:735):
  ```
  phase 0 = anything else
  phase 1 = starts with ":"
  phase 2 = starts with "//"   (or, with StripLabelLeadingSlashes, anything not "@")
  phase 3 = starts with "@"
  then element-wise on strings.Split(strings.Replace(v, ":", "."), ".")
  then whole value, then original index
  ```
  Sorting is **chunked**: a chunk ends at an element carrying a `Before` comment, so
  comments pin sort groups. Duplicates within a chunk are deleted (`uniq`).
- **`sortCallArgs` / `reorderArguments`** — `tables.NamePriority` puts `name` first.
- **Suppression comments** (`hasComment` / `isCommentAnywhere`):
  `# buildifier: leave-alone`, `# do not sort`, `# keep sorted`,
  `# disable=load-on-top`, `# disable=out-of-order-load`, `# disable=same-origin-load`.

### The LINT rules

`warn/` is 21 source files. Registration (`warn/warn.go:113-225`):

| Map | Count | Notes |
|---|---|---|
| `RuleWarningMap` | **0** | empty at HEAD |
| `FileWarningMap` | **52** | pure single-file |
| `MultiFileWarningMap` | **48** | take a `*FileReader`; read *other* files |
| `warn.AllWarnings` | **100** | union |
| `warn.DefaultWarnings` | **99** | `nonDefaultWarnings = {"unsorted-dict-items"}` |

`warn/docs/warnings.textproto` documents **104** IDs. The four documented-but-not-registered:
`attr-package-metadata` (never implemented), and **`load-on-top`, `out-of-order-load`,
`same-origin-load` — which were promoted from lints to *formatter rewrites***
(`rewrite.go:958`: *"For backward compatibility. This rewrite used to be a suppressible
warning"*).

**Auto-fixable**: 46 documented entries carry `autofix: true`; intersecting with the 100
live lints gives **43 auto-fixable lints**:

```
attr-cfg, attr-non-empty, attr-single-file, ctx-actions, ctx-args, depset-iteration,
git-repository, http-archive, integer-division, keyword-positional-params, list-append,
load, native-android, native-build, native-cc-binary, native-cc-proto,
native-java-binary, native-java-common, native-java-import, native-java-info,
native-java-library, native-java-lite-proto, native-java-package-config,
native-java-plugin, native-java-plugin-info, native-java-proto, native-java-runtime,
native-java-test, native-java-toolchain, native-proto, native-proto-common,
native-proto-info, native-proto-lang-toolchain, native-proto-lang-toolchain-info,
native-py, native-sh-binary, native-sh-library, native-sh-test, output-group,
package-name, repository-name, skylark-comment, unsorted-dict-items
```

**Not auto-fixable** (36 live, plus documented-only `attr-package-metadata`):

```
allowed-symbol-load-locations, attr-applicable_licenses, attr-license, attr-licenses,
attr-output-default, build-args-kwargs, bzl-visibility, canonical-repository,
confusing-name, constant-glob, deprecated-function, depset-items, depset-union,
dict-concatenation, dict-method-named-arg, duplicated-name, external-path, filetype,
function-docstring, module-docstring, name-conventions, native-package, no-effect,
overly-nested-depset, package-on-top, positional-args, print, provider-params,
redefined-variable, return-value, rule-impl-return, string-iteration, uninitialized,
unnamed-macro, unreachable, unused-variable
```

(Plus the `function-docstring-{header,args,return}` and `skylark-docstring` variants,
and the ~30-strong `native-cc-*` / `native-java-*` / `native-proto-*` family that all
say "X is not global anymore and needs to be loaded from @rules_Y//…".)

Lint data model:

```go
type LintMode int   // ModeWarn | ModeFix | ModeSuggest
type LinterFinding struct { Start, End build.Position; Message, URL string; Replacement []LinterReplacement }
type Finding struct {
	File *build.File; Start, End build.Position
	Category, Message, URL string
	Actionable, AutoFixable bool
	Replacement *Replacement          // {Description string; Start, End int /* bytes */; Content string}
}
```

`Replacement` is already an LSP-shaped text edit (byte offsets + content), but it is only
populated in `ModeSuggest` and **is not serialised into the JSON output** (see below).

The 48 `MultiFileWarningMap` rules are the cross-file ones: `bzl-visibility` reads the
loaded `.bzl`'s package for `# bzl-visibility` markers; `deprecated-function` reads the
definition's docstring; `positional-args` and `unnamed-macro` need the macro's `def`.
Those are the lints an LSP can serve better than a batch tool, because it already has the
graph.

### `buildifier` CLI: real runs

```
$ buildifier --version
buildifier version: 8.5.1
buildifier scm revision: v8.5.1
```

**`--format=json` requires `--mode=check`.** The invocation in the brief is rejected:

```
$ buildifier --mode=diff --lint=warn --format=json BUILD.bazel
buildifier: cannot specify --format without --mode=check
EXIT=2
```

Documented exit codes (`--help`):

```
0: success, everything went well
1: syntax errors in input
2: usage errors: invoked incorrectly
3: unexpected runtime errors: file I/O problems or internal bugs
4: check mode failed (reformat is needed)
```

With `--format=json` the process exits **0** and encodes the verdict in `"success"`.

#### The sample bad BUILD file (`/tmp/bzlbad/BUILD.bazel`)

```python
load("@rules_cc//cc:defs.bzl", "cc_library")
load(":unused.bzl", "unused_symbol")

package(default_visibility = ["//visibility:public"])

cc_library(
    name = "foo",
    srcs = ["b.cc", "a.cc"],
    deps = [
      "//bar:bar",
      ":baz",
    ],
)

cc_library(name = "foo", srcs = glob(["*.cc"]))

java_library(
    name = "j",
    srcs = ["J.java"],
)

def my_macro(name):
    print("hello " + name)
    x = 1 / 2
    native.cc_binary(name = name)
    d = {"b": 1, "a": 2}
    return d

my_macro("m")
```

#### Real output

```
$ nix shell nixpkgs#bazel-buildtools -c buildifier --format=json --mode=check --lint=warn BUILD.bazel; echo "EXIT=$?"
{"success":false,"files":[{"filename":"BUILD.bazel","formatted":false,"valid":true,"warnings":[{"start":{"line":2,"column":22},"end":{"line":2,"column":35},"category":"load","actionable":true,"autoFixable":true,"message":"Loaded symbol \"unused_symbol\" is unused. Please remove it.\nTo disable the warning, add '@unused' in a comment.","url":"https://github.com/bazelbuild/buildtools/blob/main/WARNINGS.md#load"},{"start":{"line":15,"column":19},"end":{"line":15,"column":24},"category":"duplicated-name","actionable":true,"autoFixable":false,"message":"A rule with name \"foo\" was already found on line 6. Even if it's valid for Blaze, this may confuse other tools. Please rename it and use different names.","url":"https://github.com/bazelbuild/buildtools/blob/main/WARNINGS.md#duplicated-name"},{"start":{"line":17,"column":1},"end":{"line":17,"column":13},"category":"native-java-library","actionable":true,"autoFixable":true,"message":"Function \"java_library\" is not global anymore and needs to be loaded from \"@rules_java//java:java_library.bzl\".","url":"https://github.com/bazelbuild/buildtools/blob/main/WARNINGS.md#native-java-library"},{"start":{"line":22,"column":1},"end":{"line":22,"column":19},"category":"function-docstring","actionable":true,"autoFixable":false,"message":"The function \"my_macro\" has no docstring.\nA docstring is a string literal (not a comment) which should be the first statement of a function body (it may follow comment lines).","url":"https://github.com/bazelbuild/buildtools/blob/main/WARNINGS.md#function-docstring"},{"start":{"line":23,"column":5},"end":{"line":23,"column":27},"category":"print","actionable":true,"autoFixable":false,"message":"\"print()\" is a debug function and shouldn't be submitted.","url":"https://github.com/bazelbuild/buildtools/blob/main/WARNINGS.md#print"},{"start":{"line":24,"column":9},"end":{"line":24,"column":14},"category":"integer-division","actionable":true,"autoFixable":true,"message":"The \"/\" operator for integer division is deprecated in favor of \"//\".","url":"https://github.com/bazelbuild/buildtools/blob/main/WARNINGS.md#integer-division"},{"start":{"line":24,"column":5},"end":{"line":24,"column":6},"category":"unused-variable","actionable":true,"autoFixable":false,"message":"Variable \"x\" is unused. Please remove it.","url":"https://github.com/bazelbuild/buildtools/blob/main/WARNINGS.md#unused-variable"},{"start":{"line":25,"column":5},"end":{"line":25,"column":11},"category":"native-build","actionable":true,"autoFixable":true,"message":"The \"native\" module shouldn't be used in BUILD files, its members are available as global symbols.","url":"https://github.com/bazelbuild/buildtools/blob/main/WARNINGS.md#native-build"},{"start":{"line":25,"column":5},"end":{"line":25,"column":21},"category":"native-cc-binary","actionable":true,"autoFixable":true,"message":"Function \"cc_binary\" is not global anymore and needs to be loaded from \"@rules_cc//cc:cc_binary.bzl\".","url":"https://github.com/bazelbuild/buildtools/blob/main/WARNINGS.md#native-cc-binary"},{"start":{"line":29,"column":1},"end":{"line":29,"column":14},"category":"positional-args","actionable":true,"autoFixable":false,"message":"All calls to rules or macros should pass arguments by keyword (arg_name=value) syntax.\nFound call to rule or macro \"my_macro\" with positional arguments.","url":"https://github.com/bazelbuild/buildtools/blob/main/WARNINGS.md#positional-args"}]}]}
EXIT=0
```

Pretty-printed schema (multi-file, mixing a clean and a broken file):

```json
{
    "success": false,
    "files": [
        {
            "filename": "BUILD.ok",
            "formatted": true,
            "valid": true,
            "warnings": [
                {
                    "start": { "line": 1, "column": 1 },
                    "end":   { "line": 1, "column": 11 },
                    "category": "native-cc-library",
                    "actionable": true,
                    "autoFixable": true,
                    "message": "Function \"cc_library\" is not global anymore and needs to be loaded from \"@rules_cc//cc:cc_library.bzl\".",
                    "url": "https://github.com/bazelbuild/buildtools/blob/main/WARNINGS.md#native-cc-library"
                }
            ]
        },
        { "filename": "BUILD.broken", "formatted": false, "valid": false, "warnings": [] }
    ]
}
```

`line` and `column` are both **1-based**. There is **no `replacement` field** — the
auto-fix payload is not exposed over JSON, only via `--lint=fix`.

`--mode=diff --lint=warn` (text) emits a unified diff on stdout, lints on stderr, and
exits **4**:

```diff
@@ -5,14 +5,20 @@
 cc_library(
     name = "foo",
-    srcs = ["b.cc", "a.cc"],
+    srcs = [
+        "a.cc",
+        "b.cc",
+    ],
     deps = [
-      "//bar:bar",
-      ":baz",
+        ":baz",
+        "//bar",
     ],
 )
-cc_library(name = "foo", srcs = glob(["*.cc"]))
+cc_library(
+    name = "foo",
+    srcs = glob(["*.cc"]),
+)
```

Note `"//bar:bar"` → `"//bar"` happening under *formatting*, not linting.

Config: `-config`, `BUILDIFIER_CONFIG`, or `.buildifier.json` at the workspace root;
`-config=example` prints a sample; `-config=off` disables.

### Throughput (measured)

| Command | Corpus | Wall |
|---|---|---|
| `buildifier --mode=check -r .` | `upstream/bazel`: 13,739 files, 682 BUILD/.bzl, 3.0 MB | **1.02 s** |
| `buildifier --mode=check --lint=warn -r .` | same | **1.12 s** |
| `buildifier --mode=print_if_changed <26 KB .bzl>` × 20 | one file | **148 ms** → **7.4 ms/invocation** incl. process start |

7.4 ms per spawn is fine for on-save formatting and on-save linting. It is not fine for
on-type diagnostics across a 74k-package repo.

---

## 5. starlark-go `syntax`

`upstream/starlark-go`, BSD-3-Clause, HEAD `5395d018` 2026-07-08 ("syntax: reject
excessively nested bracketed expressions"). 4,439 LOC in `syntax/`:
`scan.go` (hand-written scanner), `parse.go` (hand-written recursive descent),
`syntax.go` (AST), `quote.go`, `walk.go`, and `grammar.txt` (an EBNF reference).

The AST is the direct ancestor of buildtools' — same shape:

```go
type Node interface { Span() (start, end Position); Comments() *Comments; AllocComments() }
type Comment  struct { Start Position; Text string }
type Comments struct { Before []Comment; Suffix []Comment; After []Comment }  // Suffix: "up to 1"
type commentsRef struct{ ref *Comments }   // embedded in every node, nil until AllocComments
```

Comments are **opt-in**: `syntax.Parse(filename, src, syntax.RetainComments)`, attached
post-hoc by `p.assignComments(f)` (`parse.go:1048`). Nodes: `AssignStmt, BranchStmt,
DefStmt, ExprStmt, ForStmt, WhileStmt, IfStmt, LoadStmt, ReturnStmt` plus the expression
set. `WhileStmt` exists because go-Starlark has an opt-in `while`.

**No error recovery**: `defer p.in.recover(&err)` in every entry point;
`scanner.errorf` panics. One error, no tree.

`FileOptions{Set, While, TopLevelControl, GlobalReassign, LoadBindsGlobally, Recursion}`
are go-Starlark dialect switches, not Bazel's. There are no BUILD concepts, no `select`,
no label type.

**Relevance to this project: reference only.** Bazel does not use starlark-go; it is
the spec-conformance implementation for the Starlark *language* and a good source of
grammar test cases (`syntax/testdata/`, `syntax/grammar.txt`).

---

## 6. Bazel's own Java parser — the normative reference

`upstream/bazel/src/main/java/net/starlark/java/syntax/`, 58 files, **14,618 LOC**.
This is what actually runs when Bazel loads your BUILD file, so its error messages are
the ones users have memorised.

Contents beyond the obvious: `Lexer.java`, `Parser.java`, `Resolver.java`,
`FileLocations.java`, `Program.java`, `DocComments.java`, plus a **type system** that
did not exist a few years ago — `StarlarkType, TypeChecker, TypeTable, TypeContext,
TypeConstructor, TypeTagger, Types, TypeAliasStatement, VarStatement, CastExpression,
IsInstanceExpression`. Bazel 9 Starlark typing is being built here.

### Error recovery: real, but deliberately lossy

```java
private int errorsCount;
private boolean recoveryMode;   // stop reporting errors until next statement
```

Machinery: `syncTo(EnumSet<TokenKind>)`, `syncPast(...)`, `expect(kind)`,
`expectAndRecover(kind)` (clears `recoveryMode` on success), and
`makeErrorExpression(start, end)` — used at **14 sites** — which fabricates an
`Identifier` holding the misparsed text so the tree stays well-typed. `ParseResult`
always carries `{locs, statements, comments, errors}`; there is always a tree.

But:

```java
private void reportError(int offset, String format, Object... args) {
    errorsCount++;
    // Limit the number of reported errors to avoid spamming output.
    if (errorsCount <= 5) { errors.add(new SyntaxError(location, String.format(format, args))); }
}
private void syntaxError(int offset, TokenKind kind, Object value, String message) {
    if (!recoveryMode) { … recoveryMode = true; }
}
```

**Max 5 diagnostics per file**, and errors are suppressed while in recovery mode until
the next statement. Correct for a compiler CLI, wrong for an LSP that must decorate
every squiggle. Copy the messages, not the throttle.

### Comments

`Comment{offset, text}` with `getText()` (includes `#`, excludes newline),
`hasDocCommentPrefix()` (`#:`), `getDocCommentText()`. Collected as a **flat
`ImmutableList<Comment>`** on `StarlarkFile.getComments()`, not attached to nodes —
same as starlark-rust, unlike buildtools. `DocComments` maps global name → doc comments
and is filled in by the `Resolver`, not the parser.

### The error-message corpus worth matching verbatim

`Parser.java`:

```
indentation error
syntax error at '<tok>': expected <KIND>
expected identifier after dot
expected expression
expected a type
expected a type argument
expected at least one symbol to load
expected an indented block
expected ',', 'for' or ']'
expected '<bracket>', 'for' or 'if'
expected 'in'
expected either a literal string or an identifier
keyword argument must have form name=expr
Trailing comma is allowed only in parenthesized tuples.
Implicit string concatenation is forbidden, use the + operator
Starlark does not support Python-style generator expressions
missing else clause in conditional expression or semicolon before if
ellipsis ('...') is not allowed outside type expressions
type annotations are disallowed
duplicate type parameter
```

`checkForbiddenKeywords()` — Python keywords mapped to Starlark advice:

```
'assert' not supported, use 'fail' instead
'del' not supported, use '.pop()' to delete an item from a dictionary or a list
'import' not supported, use 'load' instead
'is' not supported, use '==' instead
'raise' not supported, use 'fail' instead
'try' not supported, all exceptions are fatal
'while' not supported, use 'for' instead
keyword '<KIND>' not supported          (for AS CLASS EXCEPT FINALLY FROM GLOBAL NONLOCAL WITH YIELD)
```

Note the last line again: **`while` and `with` are hard errors in Bazel Starlark** — and
tree-sitter-starlark parses both happily.

`Lexer.java`:

```
unclosed string literal
Tab characters are not allowed for indentation. Use spaces instead.
invalid escape sequence: \<c>. Use '\\' to insert '\'.
octal escape sequence out of range (maximum is \377)
octal escape sequence denotes non-ASCII character
string literal contains non-ASCII character
invalid hex literal / invalid binary literal / invalid float literal
floating-point literal too large
invalid character: '<c>'
```

`Resolver.java` — the semantic pass an LSP most needs to mirror, and the place where the
best diagnostics live:

```
symbol '%s' is private and cannot be imported
load statement defines '%s' more than once
load statement not at top level
cannot assign to '%s'
return statements must be inside a function
%s statement must be inside a for loop           (break/continue)
positional argument may not follow *args / **kwargs / keyword argument
keyword argument %s may not follow *args / **kwargs
duplicate keyword argument: %s
*args may not follow **kwargs
multiple *args not allowed / multiple **kwargs not allowed
type alias statement not at top level
contains syntax errors                            (on a makeErrorExpression Identifier)
```

plus spelling suggestions on undefined names:

```java
String suggestion = SpellChecker.didYouMean(name, getAllSymbols(ex.candidates));
errorf(id, "%s%s", ex.getMessage(), suggestion);
```

---

## 7. Front-end comparison

| | tree-sitter-starlark | starpls syntax | starlark-rust | buildtools `build/` | starlark-go | Bazel Java |
|---|---|---|---|---|---|---|
| Language | C (gen'd from JS) | Rust | Rust | Go | Go | Java |
| License | MIT | Apache-2.0 / MIT | Apache-2.0 | Apache-2.0 | BSD-3 | Apache-2.0 |
| Last commit | 2024-12-04 | 2025-12-03 | 2026-08-24 | 2026-08-24 | 2026-07-08 | 2026-08-24 |
| Tree kind | CST (tree-sitter) | CST (rowan) | AST | AST + comments | AST + comments | AST + comment list |
| Lossless | Yes (bytes) | Yes (bytes) | **No** | Formatter-lossless | Formatter-lossless (opt-in) | **No** |
| Comments attached to nodes | No (siblings) | Yes (trivia in tree) | No (span list) | **Yes (Before/Suffix/After)** | **Yes** | No (flat list) |
| Whitespace preserved | Yes | Yes | No | No (regenerated) | No | No |
| Error recovery | **Excellent** (measured 2-char ERROR) | Good shape, one crude recovery set | **None** (first error aborts) | **None** (panics) | **None** (panics) | Good, but ≤5 errors + suppression |
| Partial tree on error | Yes | Yes | No | No | No | Yes |
| Incremental reparse | **Yes** (`ts_tree_edit`) | No (full reparse, memoized) | No | No | No | No |
| `load()` is a node | **No** (bare `call`) | **Yes** (`LOAD_STMT/LOAD_MODULE/DIRECT_LOAD_ITEM/ALIASED_LOAD_ITEM`) | Yes (`StmtP::Load`) | Yes (`LoadStmt`) | Yes (`LoadStmt`) | Yes (`LoadStatement`) |
| BUILD-vs-bzl dialects | No | Yes (`Dialect`, `APIContext`) | Partial (`Dialect`) | **Yes (7 `FileType`s)** | No | Yes (`FileOptions`) |
| Fidelity to Bazel Starlark | Poor (accepts `while`/`with`/`match`/decorators/f-strings) | Good | Good | Good | Go dialect | **Normative** |
| Suitable as LSP front end | Fidelity/maintenance risk | **Yes** | No | No | No | n/a (JVM) |

---

## 8. Incremental analysis architecture

### 8.1 salsa

`crates.io/crates/salsa`, current **0.28.2, published 2026-08-03**. Still 0.x after eight
years; no 1.0 on the horizon.

2026 release cadence and churn:

| Version | Date | Breaking |
|---|---|---|
| 0.26.0 | 2026-02-07 | yes |
| 0.26.1 / 0.26.2 | 2026-03-20 / 2026-05-03 | |
| 0.27.0 | 2026-06-04 | yes |
| 0.27.1 | 2026-06-24 | *"Revamp tracked attribute for methods and impls to better handle lifetimes"*; adds never-change durability |
| 0.27.2 | 2026-06-25 | |
| 0.28.0 | 2026-07-12 | **yes**: *"replace `Update` with `SalsaValue` and `PartialEq`"*, *"return references by default"* |
| 0.28.1 / 0.28.2 | 2026-07-22 / 2026-08-03 | |

Three breaking minors in seven months. Budget an upgrade every ~4 months.

**Is salsa 0.x still the norm in 2026? Yes.** rust-analyzer `master`'s workspace
`Cargo.toml`:

```toml
rowan = "0.17.0"
salsa = { version = "0.28.2", default-features = false, features = [
    "rayon", "salsa_unstable", "macros", "inventory", "triomphe"
] }
```

The migration history is worth knowing because it prices the churn:

- PR **#18964** "internal: port rust-analyzer to new Salsa" — needed a bespoke
  compatibility macro (`db-ext-macro`, later vendored as
  `crates/query-group-macro`: *"A macro that mimics the old Salsa-style `#[query_group]`
  macro"*) plus upstream salsa PRs to fix an 8 GB → 4 GB memory regression.
- PR **#22831** "internal: Migrate `HirDatabase` away from `#[query_group]`; remove the
  latter", merged **2026-07-20**, +168 −884. The shim is being retired.

**starpls does not use crates.io salsa.** All three of `starpls_common`, `starpls_hir`,
`starpls_ide` pin:

```toml
salsa = { git = "https://github.com/withered-magic/salsa", package = "salsa-2022", rev = "91fdda90b344ef74e9bf35c3a5bb0fbae22ed6fb" }
```

That is the author's **personal fork of the pre-0.17 "salsa 2022" generation** — the one
with `#[salsa::jar]`, which modern salsa removed. Anyone forking starpls inherits that
fork and a migration.

starpls's salsa usage is otherwise idiomatic and a good template:

```rust
#[salsa::jar(db = Db)] pub struct Jar(Diagnostics, File, LineIndexResult, Parse, parse, line_index_query);
#[salsa::input]  pub struct File { pub id: FileId, pub dialect: Dialect, pub info: Option<FileInfo>, #[return_ref] pub contents: String }
#[salsa::tracked] pub fn parse(db: &dyn Db, file: File) -> Parse { … }
#[salsa::accumulator] Diagnostics
#[salsa::interned] LoadStmt
```

`FileId(u32)` is an opaque interned path key; the `Db` trait exposes `create_file`,
`update_file`, `load_file`, `get_file`, `resolve_path`, `list_load_candidates`,
`resolve_build_file` — filesystem access lives outside the query system, behind a trait.
That matches rust-analyzer's *"base-db doesn't know about file system and file paths"*
invariant.

### 8.2 rust-analyzer's layering, and how it maps to Bazel

From `docs/book/src/contributing/architecture.md`, the invariants that matter:

- `parser`: *"transforms one flat stream of events into another flat stream of events…
  the parser is independent of the particular tree structure and particular
  representation of the tokens."* And: **"parsing never fails, the parser produces
  `(T, Vec<Error>)` rather than `Result<T, Error>`."**
- `syntax`: *"completely independent from the rest of rust-analyzer. It knows nothing
  about salsa or LSP."* *"syntax tree is a value type… doesn't need global context."*
  *"syntax tree is built for a single file… to enable parallel parsing."* *"Syntax trees
  are by design incomplete and do not enforce well-formedness."*
- `base-db`: holds the salsa **input** queries. *"particularities of the build system are
  not the part of the ground state… `base-db` knows nothing about cargo."*
  *"`base-db` doesn't know about file system and file paths. Files are represented with
  opaque `FileId`."*
- `hir-def`/`hir-ty`: *"`ItemTree` condenses a single `SyntaxTree` into a 'summary' data
  structure, **which is stable over modifications to function bodies**."* `DefMap` is the
  module tree; `Body` is expression-level and deliberately downstream of `ItemTree`.
- `hir`: OO façade. `ide-db` + `ide*`: features. `vfs`/`vfs-notify`: the filesystem.

Mapping to Bazel:

| rust-analyzer | Bazel analogue | Notes |
|---|---|---|
| crate | **package** (a directory with a BUILD file) for `//pkg:tgt`; **module/repo** for `@repo//` | Two-level, unlike Rust |
| crate root file | `BUILD` / `BUILD.bazel` | |
| `CrateGraph` edges (from `cargo metadata`) | `MODULE.bazel` `bazel_dep`/`use_repo` + `bazel mod dump_repo_mapping`, plus intra-repo label edges from `deps = [...]` | |
| `cfg` flags | `select()` config keys, `--define`, platform constraints | |
| macro expansion | `.bzl` macro + Bazel-9 **symbolic macro** expansion | closest analogue, and the hardest |
| `ItemTree` (stable summary) | **`PackageSummary`** = {target name → (rule kind, span, label-valued attrs)}; **`BzlSummary`** = {exported symbol → (kind, signature span)} ∪ {loaded modules} | the single most important idea to port |
| `DefMap` | label namespace of a package + the repo mapping | |
| `Body` | attribute *values* / macro bodies | should be lazily computed, downstream of the summary |
| `vfs` / `vfs-notify` | workspace tree + `$(bazel info output_base)/external` | |

Two disanalogies that must be designed for:

1. **Rust's crate graph is supplied by an external tool and is small** (hundreds of
   nodes). Bazel's package graph is *discovered by evaluating the very files being
   edited* and is O(100k) nodes.
2. **`glob()` makes the ground state depend on directory listings, not just on file
   contents.** `dir_listing(dir) -> Vec<FileName>` must be a first-class salsa input, and
   `glob(pkg, pattern)` a tracked query over it. Otherwise adding a `.cc` file silently
   fails to invalidate the target's `srcs`.

The precedent for getting the graph shape right is rust-analyzer PR **#19337, "Salsify
the crate graph"**:

> make it not one giant input but multiple, for incrementality and decreased memory
> usage … changing the metadata of a crate (e.g. adding a dependency) invalidates only it.

Do not model `MODULE.bazel` as one input. Each `bazel_dep`, each repo-mapping entry, and
each package's summary should be its own salsa entity.

### 8.3 The `.bzl` fan-out problem, and how big real repos are

Published numbers:

- `bazelbuild/bazel#22233` (user report, Bazel 7 skymeld regression):
  > `Analyzing: 175831 targets (74369 packages loaded, 764940 targets configured)`

  **74,369 BUILD files** in one repository.
- `bazelbuild/bazel#29594` (2026, Bazel 9 repo-mapping perf):
  > "We run a build with RBE on Linux with about **161k targets** in our
  > `--target_pattern_file`."
  > "…a build in one of our monorepos of a fairly large amount of targets (**189k**)
  > where we noticed that 79k `RepoMappingManifest` actions were run…"
  > "Build profiles produced for large builds show **121,000+** 'Writing repo mapping
  > manifest' actions."
- Airbnb's JVM monorepo migration: ">10x more Bazel targets than Gradle projects",
  migrating from ~4,500 hand-maintained Gradle files.
- bazel.build's own marketing floor: "handling builds with 100k+ source files… user bases
  in the tens of thousands."
- Measured locally, `bazelbuild/bazel` itself: **575** `BUILD`/`BUILD.bazel` + **107**
  `.bzl` = 682 files, **3.0 MB**, in a 13,739-file tree.

Extrapolating that ~4.4 KB/file average to 74k packages gives **~330 MB of Starlark**
inside the workspace, before external repos. `output_base` is separately large:
`bazelbuild/bazel#29075` reports `du -h $(bazel info output_base)` = **1.3 GB** just to
build Bazel itself (380 MB with the 8.8 remote repo-contents cache); community reports of
11–20 GB per output_base are routine.

**The invalidation problem.** A `.bzl` in `//tools/build_defs` `load()`ed by 20k BUILD
files: naively, one keystroke in it invalidates 20k parses and 20k target lists.
Mitigations, in descending value:

1. **Firewall on the export summary.** Make `bzl_exports(file) -> BTreeMap<Name, SymbolKind>`
   a `#[salsa::tracked]` query, and make `load()` resolution depend on `bzl_exports`, not
   on `parse`. Editing a function *body* leaves `bzl_exports` byte-identical, salsa's
   backdating fires, and nothing downstream re-runs. This is exactly `ItemTree`, and
   salsa 0.28's "replace `Update` with `SalsaValue` and `PartialEq`" makes the equality
   check cheap.
2. **Never eagerly materialise target lists.** `targets_in_package(pkg)` is on-demand.
   The only feature that legitimately wants all 190k targets is `workspace/symbol`, and
   that should be served from a separate coarse index, not from the HIR.
3. **Do not evaluate Starlark.** Full evaluation of 74k BUILD files *is* a Bazel loading
   phase; even with a warm server that is tens of seconds. Approximate syntactically —
   pattern-match `rule_name(name = "…", …)` — and fall back to real evaluation only for
   the specific macro a user hovers. starpls's `--experimental_infer_ctx_attributes` and
   its symbolic-macro support (#359) are exactly this approximation.
4. **External repos get their own durability.** Files under `output_base/external` are
   read-only between fetches. Mark them `Durability::HIGH` (salsa 0.27.1 added a
   never-change durability, `#1109`) so typing in the workspace does not walk their
   dependency edges.

**Eager vs lazy indexing.** Parsing is cheap; retaining trees is not.

- Parse: 330 MB at the measured 32 MB/s ≈ **10 s single-threaded, ~1.5 s on 8 cores**.
  Affordable at startup, in the background.
- Retain: rowan green trees run roughly 5–10× source size ⇒ **1.6–3.3 GB** for 74k
  packages. Not affordable.
- Lint: 99 buildifier warnings across the tree is 1.1 s as a batch (measured on a
  682-file repo; ~2 min extrapolated to 74k files at the same per-byte rate). Not
  affordable per keystroke.

So the shape is a **two-tier index**:

- **Tier 1, eager, always resident**: a lexer-only (no tree) scan producing
  `target name → (FileId, TextRange, rule kind)` and
  `exported .bzl symbol → (FileId, TextRange)`. At ~100 bytes/entry that is **~20 MB at
  190k targets** — trivially resident, and enough for `workspace/symbol`, label
  completion, and goto-definition on a label.
- **Tier 2, lazy, LRU**: full rowan CSTs + HIR for open files, their transitive `load()`
  closure, and a bounded LRU. Everything else is reconstructed on demand from tier 1's
  `(FileId, TextRange)`.

That is also what rust-analyzer actually does; `ItemTree` is tier 1 in disguise.

### 8.4 File watching

What must be watched:

| Pattern | Why | Cost |
|---|---|---|
| `**/BUILD`, `**/BUILD.bazel` | creating one **creates a package**, changing label resolution for everything under it | high fan-out |
| `**/*.bzl` | `load()` graph | high fan-out |
| `MODULE.bazel`, `*.MODULE.bazel`, `MODULE.bazel.lock` | repo graph, repo mapping, `use_repo` | rare, cheap |
| `WORKSPACE`, `WORKSPACE.bazel`, `WORKSPACE.bzlmod`, `REPO.bazel` | legacy repo graph | rare |
| `vendor/**` manifests | vendored repo resolution | rare |
| `.bazelrc`, `**/*.bazelrc`, `.bazelversion`, `.bazelignore` | flags change `select()` resolution and which dirs are packages | rare, but must invalidate everything |
| **directory create/delete, anywhere** | `glob()` results and package boundaries depend on the *listing* | **the expensive one** |
| `$(bazel info output_base)/external/**/{BUILD*,*.bzl}` | cross-repo goto-definition | changes only on refetch |

Costs and failure modes:

- LSP `workspace/didChangeWatchedFiles` with `**/BUILD*` glob patterns pushes the watch
  to the client. That is correct and cheap for the server, but the client **will not
  watch outside the workspace folders** — so `output_base/external` needs a server-side
  watcher or a poll. Since `output_base` is by definition outside the tree, plan for
  server-side.
- On Linux, `notify`/inotify needs **one watch per directory**. 74k packages ⇒ 74k+
  watches against a default `fs.inotify.max_user_watches` of 8,192 / 65,536 / 524,288
  depending on distro. rust-analyzer hits this routinely; its `vfs-notify` crate exists
  to manage it. On macOS, FSEvents is recursive and cheap but coalesces events and
  reports paths only (no "what changed"), so you re-stat.
- Practical mitigation: watch the workspace root **non-recursively** plus the specific
  package directories actually loaded (lazy watch registration, grown as the load graph
  is explored); treat `output_base/external` as immutable and re-scan only when
  `MODULE.bazel.lock` or `bazel info output_base` changes.
- **Data point on what happens if you skip it**: starpls has **no watcher at all** —
  no `didChangeWatchedFiles` registration, no `notify` dependency anywhere in
  `crates/`. It lazily reads files from disk via `Db::load_file` and debounces analysis
  by 250 ms (`--analysis_debounce_interval`, `crates/starpls/src/commands/server.rs:42`).
  Consequence: a `bazel fetch` or a `git checkout` that rewrites files the server has
  already cached leaves it stale until restart.

---

## 9. Recommendation

### Front end: fork starpls's `starpls_lexer` / `starpls_parser` / `starpls_syntax`

Write a hand-written, lossless, error-recovering recursive-descent parser producing a
**rowan** CST, starting from starpls's three syntax crates (dual Apache-2.0/MIT, so
compatible with anything) rather than greenfield. They are ~6.3 kLOC including tests and
are already the rust-analyzer design done correctly for Starlark, with `load()` as a
first-class node and a BUILD/bzl `Dialect` split.

Why not the alternatives:

- **tree-sitter-starlark** fails on four independent grounds: (a) *fidelity* — it is
  tree-sitter-python with 15 rules overridden, still accepting `while`, `with`, `match`,
  decorators, `assert`, f-strings, sets, all of which Bazel rejects outright; (b)
  *modelling* — no `load` node, no BUILD concepts, only `.bzl` in `file-types`, and a
  `queries/tags.scm` declared in the manifest that does not exist; (c) *maintenance* —
  last commit 2024-12-04, crates.io 1.3.0 unchanged for 20 months against
  `tree-sitter 0.26.13`, and open issue #9 (2025-10-11) is precisely "the Python grammar
  we inherit from has moved and we are now incompatible"; (d) the trailing-comma
  losslessness bug #7 is open and directly affects formatting. Its error recovery and
  20–32 MB/s throughput are genuinely excellent — but a hand-written event parser gets
  recovery too, and full reparse of a 4 KB BUILD file is ~0.2 ms, so incrementality buys
  nothing here.
  - *Worth keeping anyway*: use it behind a dev-only feature flag as a **differential
    fuzz oracle** — parse a large corpus with both and compare token coverage and error
    positions. Cheap, and it will find real bugs.
  - *Worth stealing regardless*: the `(ERROR "." @trailing-dot .)` idea from
    tilt-dev/starlark-lsp — treat parser error nodes as completion triggers.
- **starlark-rust**, **starlark-go** and **buildtools' yacc parser** are all disqualified
  by "first syntax error aborts the parse". Meta's own `starlark_lsp` documents the
  consequence in its source: a `last_valid_parse: RwLock<HashMap<LspUri, Arc<LspModule>>>`
  cache serving stale symbol locations while you type. That is unacceptable.
- **Bazel's Java parser** is the specification, not an implementation choice (JVM). Port
  its message strings verbatim; do **not** port its 5-error cap or its `recoveryMode`
  suppression.

Changes to make when forking starpls's parser:

1. Replace the single `STMT_RECOVERY = {NEWLINE}` with per-context recovery sets:
   `{COMMA, CLOSE_PAREN}` inside argument lists, `{COMMA, CLOSE_BRACK}` inside lists,
   `{COMMA, CLOSE_BRACE}` inside dicts, `{DEDENT, NEWLINE}` inside suites. Argument
   lists are where BUILD-file editing actually happens.
2. Emit **non-zero-width** error ranges (currently `TextRange::new(pos, pos)`).
3. Widen `SyntaxKindSet` off `u128` — token kinds already reach index 100 of 128 and
   `SyntaxKindSet::new` has no bounds check.
4. Upgrade `rowan 0.15.11 → 0.17.0`.
5. Add BUILD-specific nodes on top of the existing set where they earn their keep:
   nothing structural, but tag `CALL_EXPR` whose callee resolves to a rule so the IDE
   layer does not re-derive it, and keep the `LOAD_*` family.
6. Port the Bazel `Parser.java` / `Lexer.java` / `Resolver.java` message strings
   verbatim, including `checkForbiddenKeywords` and `SpellChecker.didYouMean`.

### Formatting: shell out to buildifier. Do not reimplement.

Byte-for-byte agreement with buildifier is a hard requirement (it is what CI runs), and
the algorithm is **position-dependent**, not width-based: `useCompactMode` compares
original source line numbers, so a Prettier-style printer over a position-free AST
cannot match it. Plus "format" includes 15 semantic rewrites (label canonicalisation,
list sorting by a 4-phase key, load merging/sorting/hoisting, argument reordering by
`tables.NamePriority`) that are all active with `--lint=off`.

Options, in order:

1. **Spawn the user's `buildifier`** for `textDocument/formatting`. Measured 7.4 ms per
   invocation. Resolve the binary from the repo's own pin (many repos vendor a
   `buildifier` target or a `.bazelversion`-adjacent pin) so versions match; fall back to
   `$PATH`. This is the only option that is correct by construction.
2. **Link `buildtools` as a Go c-archive** via cgo. Removes the spawn, keeps byte
   identity, costs you a Go runtime inside the LSP binary and a much harder build.
3. **Port `build/print.go` + `build/rewrite.go` + `tables/` to Rust** (~2,650 LOC plus
   tables). Only worth it for range formatting or on-type formatting. Non-negotiable
   constraints if you do: carry `Position{Line, LineRune, Byte}` on every node; preserve
   `ForceCompact`/`ForceMultiLine`/`TripleQuote`/`StringExpr.Token`; port all 15 rewrites
   and the `IsSortableListArg`/`NamePriority`/`IsLabelArg`/`SortableDenylist` tables and
   their `-tables`/`-add_tables`/`.buildifier.json` overrides; port the comment-chunked
   sort and the `# do not sort` / `# keep sorted` / `# buildifier: leave-alone` /
   `# disable=…` suppressions.

Whichever you pick, add a CI job that round-trips a large corpus (the Bazel repo, plus
`rules_*`) through your formatter and the pinned buildifier and diffs bytes.

### Lints: shell out for buildifier's 100; own only what needs the graph

`buildifier --mode=check --format=json --lint=warn` on save. Note the practicalities:

- `--format=json` **requires** `--mode=check`; `--mode=diff --format=json` is a usage
  error (exit 2).
- With `--format=json` the process exits 0; read `"success"` / per-file `"valid"`.
- `"valid": false` means a syntax error and **zero warnings** — so on a syntactically
  broken buffer you get nothing from buildifier and must fall back to your own parser's
  recovered diagnostics. That is another argument for owning the parser.
- The JSON carries `autoFixable` but **not** the replacement text. Code actions for the
  43 auto-fixable lints must be implemented as "copy buffer to temp, run
  `buildifier --lint=fix`, diff, offer as a `WorkspaceEdit`" — or via FFI in `ModeSuggest`,
  where `Replacement{Start, End (byte), Content}` is already LSP-shaped.
- The 48 `MultiFileWarningMap` rules (`bzl-visibility`, `deprecated-function`,
  `positional-args`, `unnamed-macro`, the `native-*` family) need to read other files.
  Those are the ones an LSP can serve *better* than the batch tool, because it already
  has the load graph resident — a good later-phase differentiator.

Own natively: unresolved labels, unknown attributes, missing `load`, `load` cycles,
visibility violations, and `.bzl` type errors. Those need the analysis DB and buildifier
cannot produce them.

### Analysis: rust-analyzer's layering, crates.io salsa 0.28, rowan 0.17

```
lexer                      no deps
parser        → lexer      tree-agnostic events; never fails; (T, Vec<Error>)
syntax        → parser     rowan CST + typed AST; knows nothing about salsa or LSP
base-db       → syntax     salsa inputs: file text, dir listings, bazel info, MODULE resolution
hir           → base-db    BzlSummary / PackageSummary (the ItemTree analogue), label resolution, load graph
ide           → hir        features
server        → ide        LSP glue, vfs, watchers
```

Concrete decisions:

- **salsa 0.28.2 from crates.io**, not starpls's `salsa-2022` fork. Accept a breaking
  upgrade roughly every four months; rust-analyzer's `query-group-macro` history shows
  the cost is real but bounded.
- **Directory listings are inputs.** `dir_listing(DirId) -> Arc<[FileName]>` as a
  `#[salsa::input]`; `glob(pkg, pattern)` as a tracked query over it. Without this,
  `glob()` is silently wrong.
- **`bazel info`, `bazel mod dump_repo_mapping`, and the builtins proto are inputs with
  HIGH durability**, refreshed on `MODULE.bazel.lock` change, never on keystroke.
  Everything under `output_base/external` gets the same treatment.
- **Split the repo graph into per-module inputs** (rust-analyzer #19337's lesson), never
  one blob.
- **`BzlSummary` is the firewall.** Everything downstream of a `load()` depends on the
  summary, never on the parse tree. Derive `PartialEq` on it and let salsa backdate.
- **Two-tier index** as in §8.3: eager lexer-only symbol index (~20 MB at 190k targets),
  lazy CSTs behind an LRU.
- **Watch lazily**: client-side `didChangeWatchedFiles` for `**/BUILD*`, `**/*.bzl`,
  `MODULE.bazel*`, `WORKSPACE*`, `REPO.bazel`, `*.bazelrc`, `.bazelignore`; server-side
  `notify` only for the specific package dirs in the loaded set plus `output_base`;
  budget for inotify watch exhaustion on Linux.
- **Debounce** the cross-file passes (starpls uses 250 ms; that is a reasonable default)
  and rely on salsa cancellation for the rest.

### Tradeoffs, stated honestly

- **The hand-written parser is the biggest up-front cost** — call it 3–4 kLOC of new/forked
  code, and you own every Bazel language change forever (Bazel 9 is actively adding
  Starlark *types*: `TypeAliasStatement`, `VarStatement`, `CastExpression`,
  `IsInstanceExpression` all landed in `net/starlark/java/syntax`). tree-sitter would
  have been ~0 LOC. You are buying fidelity to the Bazel dialect, `load()` as a node,
  Bazel-identical error messages, and independence from tree-sitter-python's release
  schedule.
- **salsa 0.x means a breaking upgrade roughly every four months.** There is no 1.0 and
  no sign of one. The mitigating fact is that rust-analyzer is on 0.28.2 and drives the
  churn, so the upgrade path is always walked by someone else first.
- **Shelling out to buildifier is a hard external dependency and a process spawn per
  format request.** The alternative — a Rust reimplementation — carries a permanent
  byte-for-byte divergence risk against the tool CI runs, which is strictly worse.
- **Not evaluating Starlark means completion and goto inside heavily macro-ised BUILD
  files will be incomplete.** The escape hatch is optional `bazel query`-backed
  enrichment behind a flag, at the price of contending for the Bazel server lock —
  a hazard starpls's own README already warns users about.
- **A 74k-package repo will not fit in memory as syntax trees.** The two-tier design is
  not an optimisation, it is a correctness constraint on the memory budget; design for it
  from day one rather than retrofitting.
