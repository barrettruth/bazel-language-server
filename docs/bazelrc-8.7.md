# Bazelrc 8.7 language-server specification

- Status: normative for `bazel-language-server`
- Last reviewed: 2026-08-31
- Language baseline: upstream Bazel 8.7.0, commit
  `a6d8d66737d56766c0f5377c62ccf2ee562a1a47`

This document defines the Bazelrc behavior the server may rely on and the LSP
answers it provides. Bazel's implementation is authoritative where this text
is ambiguous. The public [support matrix](../site/src/pages/bazelrc.mdx)
summarizes this contract; it does not extend it.

## Compatibility policy

The syntax and configuration model are pinned to upstream Bazel 8.7.0 and are
advertised as Bazel 8.7 support. A configured executable whose numeric release
is 8.7 may provide native flag metadata. A vendor suffix changes neither the
grammar contract nor the supported version line.

The server does not:

- select a nearest catalog for another Bazel release;
- fall back to a bundled flag catalog;
- infer older or newer grammar from the configured executable;
- claim vendor-only grammar or configuration behavior;
- implement Bazel 9.0's `BAZELRC` environment list.

Conditional imports in the workspace model are evaluated against the
normative `8.7.0` baseline. They are not evaluated against a configured vendor
build label. In particular, SemVer considers `8.7.0-imc2` lower than `8.7.0`;
behavior that depends on that suffix is outside this contract.

## Ten invariants

1. Every request uses one immutable `Documents`, `Index`, configuration, and
   flag-catalog snapshot; a handler performs no filesystem or Bazel work.
2. Bazelrc uses the native client's byte grammar, never the Starlark parser,
   buildifier, or shell tokenization.
3. Physical ranges survive global continuation deletion, including CRLF and
   tokens spanning physical lines.
4. The workspace configuration graph has one root, the workspace `.bazelrc`,
   and imports expand depth-first at their textual position.
5. The server models Bazel invoked from the workspace root, so ordinary
   relative imports resolve there, never against the importing file.
6. An argless ordinary line is not an entry, and therefore cannot declare an
   empty named configuration.
7. `startup` and unknown command sections cannot declare or select named
   configurations.
8. Catalog absence proves only that a spelling is not reported by the exact
   Bazel 8.7 native catalog; it never proves that the spelling is invalid.
9. Native completion and hard semantic errors require an exact numeric-8.7
   catalog; structural answers remain available without one.
10. Open buffers replace saved graph-member contents for request answers;
    unsaved imports can reuse only targets present in the published filesystem
    graph and never mutate that graph.

## Source authorities

| Concern | Bazel 8.7 source authority |
| --- | --- |
| Physical files and import expansion | `src/main/cpp/rc_file.cc`, `RcFile::ParseFile` |
| Tokenization | `src/main/cpp/util/strings.cc`, `GetNextToken` and `Tokenize` |
| Rc discovery and startup options | `src/main/cpp/option_processor.cc` |
| `%workspace%` resolution | `src/main/cpp/workspace_layout.cc` |
| Command scopes | `src/main/java/com/google/devtools/build/lib/runtime/BlazeOptionHandler.java` |
| Named configurations | `src/main/java/com/google/devtools/build/lib/runtime/ConfigExpander.java` |
| Conditional versions | `src/main/cpp/sem_ver.cc` and `BazelVersionMatchesCondition` |
| Native flag metadata | `src/main/protobuf/bazel_flags.proto` |

Relevant upstream tests are
`src/test/cpp/{rc_file,rc_options}_test.cc`,
`src/test/cpp/util/strings_test.cc`, and
`src/test/java/com/google/devtools/build/lib/runtime/BlazeOptionHandlerTest.java`.

## File classification and discovery

The exact filename `.bazelrc` and any filename ending in `.bazelrc` are
Bazelrc documents without graph context. Any regular file reached by an import
from the workspace graph is also a Bazelrc document, regardless of its name.
An unrelated open file such as `bazel.rc` is not classified as Bazelrc until a
published import graph identifies it.

The configuration snapshot starts at the workspace root `.bazelrc`. If it is
absent, the published graph is empty and ready. The snapshot follows only
imports reachable from that root. It does not reconstruct:

1. `/etc/bazel.bazelrc`;
2. `$HOME/.bazelrc`;
3. explicit `--bazelrc` arguments;
4. `/dev/null` termination of explicit rc discovery;
5. `--nosystem_rc`, `--noworkspace_rc`, `--nohome_rc`, or full
   `--ignore_all_rc_files` discovery state.

Any recognized open Bazelrc buffer receives parser and structural LSP answers,
whether or not it is reachable from the workspace root.

## Logical lines and tokens

Before splitting lines, the parser deletes every `\\\r\n` sequence and then
every `\\\n` sequence from the complete file. Quotes and comments do not stop
this deletion. Whitespace after a backslash prevents continuation.

Each resulting logical line first loses leading and trailing space, TAB, LF,
vertical TAB, form feed, and CR bytes. This happens before quote and backslash
processing, so escaped edge whitespace is stripped with its preceding
backslash left dangling. The tokenizer then applies these rules:

- space, TAB, CR, and LF delimit tokens;
- an unquoted, unescaped `#` begins a comment, including in the middle of a
  token;
- single and double quote bytes are removed;
- quoted and unquoted fragments concatenate into one token;
- a backslash removes itself and quotes the following byte, inside or outside
  quotes;
- an empty quoted fragment produces no token;
- an unclosed quote and a dangling backslash are accepted.

The first token is the key and remaining tokens are independent options. An
ordinary line with fewer than two tokens is discarded. The parser retains
physical UTF-8 ranges for logical lines, tokens, and comments.

## Directives and imports

The accepted directive forms and arities are exact:

```text
import PATH
try-import PATH
try-import-if-bazel-version CONDITION PATH
```

`CONDITION` is one token consisting of `<`, `<=`, `>`, `>=`, `==`, `!=`, or
`~` immediately followed by a version. Comparison operators require full
SemVer. `~` also accepts a major or major-minor version: `~8` covers 8.x,
whereas `~8.2` and `~8.2.4` both end at 8.3.0. Build metadata does not affect
precedence.

A false conditional import does not resolve or read its path. A true
conditional import has optional-import read behavior. `import` reports read
failure; either optional form suppresses read failure, but not malformed
imported contents or an active-stack cycle.

Imports expand depth-first where written. Reaching the same canonical file by
two independent active paths replays its entries and produces a warning. Re-entering a
file on the active stack is an error. Bazel 8.7 has no import-depth cap.

Only the exact `%workspace%/` prefix is substituted. Absolute paths remain
absolute. Bazel itself leaves every other path relative to its client process
working directory; the server's single workspace model fixes that directory
at the workspace root. A Bazel invocation launched from a subdirectory may
therefore resolve the same spelling differently. Environment variables,
`${...}`, and `~` are not expanded.

## Commands and rc scopes

The recognized names are:

```text
always common startup analyze-profile aquery build canonicalize-flags clean
config coverage cquery dump fetch help info license mobile-install mod
print_action query run shutdown sync test vendor version
```

Unknown command bases produce a warning and do not contribute flags,
configuration declarations, or references.

Effective command inheritance is:

- `coverage` inherits `test`, which inherits `build`;
- `cquery`, `fetch`, and `vendor` inherit `test` and `build`;
- `aquery`, `canonicalize-flags`, `clean`, `config`, `info`,
  `mobile-install`, `print_action`, `run`, and `test` inherit `build`;
- all other concrete commands use only their exact command scope;
- `always` and `common` can contribute to every non-startup command.

For navigation from an `always` or `common` entry, all potentially applicable
concrete declarations are retained because the invoked command is unknown.

## Named configurations

An entry key containing `:` is a configuration body only when its base is a
recognized non-startup command or rc scope and the line has at least one
option. `startup:name`, unknown-command sections, and argless sections do not
declare configurations.

At top level both `--config=name` and `--config name` create references. Inside
a named configuration body Bazel's recursive expander treats any token whose
first eight bytes are `--config` and which contains `=` as a reference using
the bytes after the first `=`. Thus `--configuration=name` is expanded before
native option parsing later rejects that spelling. Any such prefix without an
`=` is a recursive joined-form error; split spelling is not accepted.

Configuration completion and go-to-definition include declarations applicable
through command inheritance. Every open Bazelrc document replaces the saved
declarations from the same path. Other saved declarations come from the
published import graph. The server warns, rather than errors, when a referenced
name is absent because system, home, explicit, and unsaved imported rc layers
may define it.

For every concrete Bazel command, the server builds the applicable named-
configuration graph from `always`, `common`, inherited, and exact-command
declarations. It reports active-chain cycles as errors, repeated expansion as
a warning, and chains of ten or more configurations as a warning. Cycle state
is branch-local, every repeated occurrence is traversed, and one physical
finding is not duplicated merely because several command scopes expose it.

The server does not calculate or display a final effective option sequence and
does not auto-select a platform configuration from
`--enable_platform_specific_config`. References absent from the workspace
graph remain qualified warnings because omitted rc layers can define them.

## Native flag catalog

When the configured executable reports numeric major 8 and minor 7, the Bazel
actor runs:

```text
bazel --ignore_all_rc_files help flags-as-proto
```

It decodes Bazel 8.7 `FlagInfo` fields through field 16: canonical name,
negative form, documentation, command list, abbreviation, multiplicity,
effect and metadata tags, documentation category, value requirement, old
name, deprecation warning, default, expansion, converter, and enum values.

The catalog omits Bazel's `INTERNAL` options. The command list already includes
Bazel's inherited applicability. Supported native spellings are canonical
long names, negative long names, exact one-character abbreviations, negative
abbreviations in `-x-` form, old names, and negative old names. Completion does
not offer old names and filters `UNDOCUMENTED`, `HIDDEN`, and `NO_OP` flags.

`common` completion is the union of flags reported for any non-startup
command. `always` completion is the intersection across the catalog's reported
non-startup command universe. Concrete command completion uses the catalog's
exact command list.

Enum membership follows Bazel's `EnumConverter` and is ASCII-case-insensitive;
completion presents the canonical values reported by the catalog.

Recognizable Starlark build-setting forms beginning with `--//`, `--@`,
`--no//`, or `--no@` bypass native lookup. The catalog does not resolve
`--flag_alias`, external rc aliases, or vendor-only flags omitted from the
binary's own output.

## Diagnostics

Diagnostics follow proof strength.

Errors:

- malformed directive arity or version conditions;
- required-import read failures and active import cycles;
- a `--config`-prefixed token without `=` in a named configuration body;
- an exact catalog says a native option is outside the current rc scope;
- a catalogued required-value option has no joined or following value;
- a catalogued negative boolean spelling has a value.
- a value is absent from an exact catalog's nonempty enum-value set;
- an active named-configuration expansion cycle.

Warnings:

- unknown Bazel 8.7 command or rc scope;
- repeated imports;
- repeated named-configuration expansion;
- a named-configuration expansion chain of ten or more configurations;
- a configuration name absent from the published workspace graph;
- a native-looking spelling absent from the exact catalog;
- old option names and catalogued deprecations.

Information:

- catalogued `UNDOCUMENTED`, `HIDDEN`, or `NO_OP` status.

Import-graph findings from disk are published for an open document only while
its text still equals the indexed text. Syntax and catalog findings always use
the current buffer. Configuration-absence warnings begin only after a
workspace graph has been published.

The server does not validate converter-specific non-enum values, Starlark
build-setting values, aliases, or vendor behavior.

## LSP surface

For Bazelrc documents the server provides:

- completion for commands, directives, applicable configuration names, and
  exact-catalog native flag spellings and enum values;
- completion for eligible regular workspace import paths, using at most 512
  matching items, explicit single-line edits, and Bazel-safe token quoting;
- hover for commands, rc scopes, configurations, imports, and native flags;
- go-to-definition for active loaded imports and configuration references;
- document links for active loaded imports;
- references and document highlights for configuration names;
- document and workspace symbols for configuration declarations;
- conservative workspace rename for a declared configuration, provided the
  replacement is a nonempty bare token fragment and does not collide with
  another declared configuration;
- semantic tokens for directives, version conditions, paths, keys, options,
  and comments;
- folding for continued logical lines and consecutive comment-only lines;
- token, logical-line, and file selection ranges;
- current-buffer and published-graph diagnostics.

The Bazelrc request router returns no results for implementation, code lenses,
inlay hints, and formatting. Completion and rename always provide explicit
edits for text whose client-side word boundaries would otherwise be ambiguous.

## Formatting contract

Bazel 8.7 defines a byte tokenizer but no canonical Bazelrc layout. Global
continuation deletion, quoted-fragment concatenation, mid-token comments,
accepted unfinished quotes, CRLF, and whitespace adjacent to a backslash make
ordinary whitespace normalization capable of changing the token stream. The
server therefore treats whole-document formatting as an intentional
non-feature and returns no edits; Bazelrc never reaches buildifier.

Operations that must synthesize one complete token use a double-quoted form
with backslash and double-quote bytes escaped. LF and NUL are not representable
by that encoder. This local token contract supports completion without
implying a whole-document style.

## Publication and invalidation

The watch thread is the sole writer of the workspace configuration snapshot.
The Bazel actor is the sole writer of the flag catalog. The watch snapshot
contains a bounded, deterministic list of regular workspace files for import
completion. A saved graph member, candidate path, `.bazelignore`, or package-
tree change invalidates the relevant configuration state; imported files with
arbitrary names are not sent through the BUILD-file index. Imported files
outside the workspace are loaded into a snapshot but are outside the recursive
watch and therefore require a manual reindex after an external edit. Each publisher
writes an immutable snapshot and sends one bounded, coalesced wake to the
protocol loop.

The protocol loop captures documents, static/evaluated index, configuration,
and catalog together for a request. Publication reschedules diagnostics for
open Bazelrc documents. No request handler walks the filesystem, invokes
Bazel, or reloads a newer snapshot from a queued worker.

## Conformance corpus

Reduced source-derived fixtures own byte grammar, import ordering, command
inheritance, configuration applicability, and catalog spellings. An untracked
oracle may execute a verified 8.7-compatible binary in isolated temporary
workspaces to minimize disagreements before fixtures are committed.

Roadrunner is a private integration corpus, not a normative source. Its
reachable Bazelrc graph checks realistic import depth, configuration volume,
Starlark settings, and exact-catalog LSP publication. Personal rc files and
vendor artifacts remain untracked; only minimized, upstream-8.7 conclusions
enter the repository.
