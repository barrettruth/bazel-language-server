export const bazelrcBehaviorReviewed = "2026-08-31";
export const bazelrcSourceCommit = "a6d8d66737d56766c0f5377c62ccf2ee562a1a47";

export type BazelrcBehaviorEntry = {
  topic: string;
  behavior: string;
};

export type BazelrcBehaviorSection = {
  area: string;
  rows: BazelrcBehaviorEntry[];
};

export const bazelrcBehavior: BazelrcBehaviorSection[] = [
  {
    area: "Version and files",
    rows: [
      {
        topic: "Upstream Bazelrc language",
        behavior: "Pinned to upstream Bazel 8.7.0.",
      },
      {
        topic: "Native flag catalog",
        behavior:
          "Only a configured executable reporting numeric major 8 and minor 7.",
      },
      {
        topic: "Structural editing with another Bazel version",
        behavior:
          "The 8.7 parser remains active; compatibility with another release is not claimed.",
      },
      {
        topic: "Nearest-version or bundled catalog fallback",
        behavior:
          "No catalog loads unless the executable reports numeric 8.7; no nearest or bundled fallback is used.",
      },
      {
        topic: "Vendor-only grammar and configuration semantics",
        behavior:
          "A vendor-suffixed numeric 8.7 executable supplies its flags; grammar and configuration semantics remain upstream 8.7.",
      },
      {
        topic: ".bazelrc and *.bazelrc classification",
        behavior: "The filename must be exact or end in .bazelrc.",
      },
      {
        topic: "Arbitrary imported filenames",
        behavior:
          "A regular file becomes Bazelrc when reached from the workspace import graph.",
      },
      {
        topic: "Unrelated open files such as bazel.rc",
        behavior:
          "Not classified as Bazelrc without a .bazelrc suffix or published import-graph membership.",
      },
      {
        topic: "Starlark parsing, buildifier formatting, or lint",
        behavior:
          "Bazelrc remains an independent native-client language and is never sent to buildifier.",
      },
    ],
  },
  {
    area: "Logical grammar",
    rows: [
      {
        topic: "LF and CRLF continuation deletion",
        behavior:
          "Deleted globally before line splitting, including in quotes and comments.",
      },
      {
        topic: "Native whitespace delimiters",
        behavior:
          "Space, TAB, CR, and LF delimit; logical-line edges also strip vertical TAB and form feed.",
      },
      {
        topic: "Mid-token comments",
        behavior: "An unescaped, unquoted # ends the logical line.",
      },
      {
        topic: "Single/double quotes and fragment concatenation",
        behavior: "Quote bytes are removed; adjacent fragments form one token.",
      },
      {
        topic: "Backslash byte escaping",
        behavior: "Inside and outside quotes; no shell expansion is performed.",
      },
      {
        topic: "Unclosed quotes and dangling backslashes",
        behavior: "Accepted as Bazel's native tokenizer accepts them.",
      },
      {
        topic: "Empty quoted tokens",
        behavior: "Discarded rather than retained as empty arguments.",
      },
      {
        topic: "Physical ranges across continuations",
        behavior:
          "Editor ranges map logical tokens back to the original UTF-8 bytes.",
      },
      {
        topic: "Argless ordinary entries",
        behavior:
          "Bazel 8.7 discards them; they declare no empty configuration.",
      },
    ],
  },
  {
    area: "Imports and discovery",
    rows: [
      {
        topic: "import PATH",
        behavior: "Exact arity; read failures are errors.",
      },
      {
        topic: "try-import PATH",
        behavior: "Exact arity; read failures are quiet.",
      },
      {
        topic: "try-import-if-bazel-version CONDITION PATH",
        behavior: "Bazel 8.7 SemVer grammar, evaluated against baseline 8.7.0.",
      },
      {
        topic: "Configured vendor label in conditional imports",
        behavior: "Conditions do not use suffixes such as 8.7.0-imc2.",
      },
      {
        topic: "Depth-first textual import expansion",
        behavior: "No later-version import-depth cap.",
      },
      {
        topic: "Diamond import replay",
        behavior: "Entries replay and the repeated import receives a warning.",
      },
      {
        topic: "Active import cycle detection",
        behavior: "Reported as an error.",
      },
      {
        topic: "Exact %workspace%/ substitution",
        behavior: "Other %workspace% spellings are literal paths.",
      },
      {
        topic: "Absolute and workspace-root-relative paths",
        behavior:
          "The server models the Bazel client working directory as the workspace root.",
      },
      {
        topic: "Per-invocation subdirectory working directory",
        behavior:
          "Relative imports resolve from the workspace root; Bazel launched below it may resolve the same spelling differently.",
      },
      {
        topic: "Live refresh for imports outside the workspace",
        behavior:
          "They load into snapshots, but external edits require manual reindexing.",
      },
      {
        topic: "Containing-file-relative imports",
        behavior:
          "Relative imports resolve from the workspace root, never from the containing file.",
      },
      {
        topic: "Environment-variable or tilde expansion",
        behavior: "$VAR, ${VAR}, and ~ remain literal.",
      },
      {
        topic: "Workspace .bazelrc graph",
        behavior: "The sole graph root; all reachable imports are indexed.",
      },
      {
        topic: "System, home, and explicit --bazelrc layers",
        behavior: "Not reconstructed by the language server.",
      },
      {
        topic: "Bazel 9.0 BAZELRC environment list",
        behavior: "Not read because it is outside the 8.7 contract.",
      },
      {
        topic: "New unsaved import targets",
        behavior:
          "Open graph members may reuse published targets; a new target loads after save and refresh.",
      },
    ],
  },
  {
    area: "Commands and configs",
    rows: [
      {
        topic: "Bazel 8.7 commands and rc scopes",
        behavior: "Static exact list, including always, common, and startup.",
      },
      {
        topic: "Command inheritance",
        behavior:
          "Build/test/coverage and other 8.7 inheritance is transitive.",
      },
      {
        topic: "Unknown command sections",
        behavior:
          "Warned; excluded from config indexing and flag completion/checks. Native hover may still identify a spelling.",
      },
      {
        topic: "Named configuration declarations",
        behavior: "Recognized, non-startup base plus at least one option.",
      },
      {
        topic: "startup:name configurations",
        behavior:
          "Not indexed as named configurations because startup parsing cannot consume command --config.",
      },
      {
        topic: "Top-level --config=name and --config name",
        behavior: "Both spellings create configuration references.",
      },
      {
        topic: "Nested --config=name",
        behavior:
          "Included in completion, navigation, and absence diagnostics.",
      },
      {
        topic: "Bazel's recursive --config prefix quirk",
        behavior:
          "Named bodies expand any token beginning --config through its first =, before native parsing.",
      },
      {
        topic: "Nested split --config name",
        behavior: "Bazel 8.7 rejects it; the server reports an error.",
      },
      {
        topic: "Applicable configuration completion",
        behavior:
          "Follows command inheritance and all declarations in the graph.",
      },
      {
        topic: "Open-buffer declaration overlays",
        behavior: "An open file replaces declarations from its saved path.",
      },
      {
        topic: "Configuration go-to-definition",
        behavior: "Returns every applicable saved or open declaration.",
      },
      {
        topic: "Effective option/config expansion",
        behavior:
          "Graph analysis is supported; a final option sequence is not rendered.",
      },
      {
        topic: "Configuration expansion-cycle diagnostics",
        behavior: "Branch-local active-chain cycles are errors.",
      },
      {
        topic: "Repeated/deep expansion warnings",
        behavior:
          "Every occurrence is traversed; chains of ten or more configurations warn.",
      },
      {
        topic: "Automatic platform configuration selection",
        behavior: "Does not evaluate --enable_platform_specific_config.",
      },
    ],
  },
  {
    area: "Native flags",
    rows: [
      {
        topic: "help flags-as-proto acquisition",
        behavior: "Runs in the Bazel actor with all rc files ignored.",
      },
      {
        topic: "Bazel 8.7 proto fields 1–16",
        behavior:
          "All reported documentation and semantic metadata are decoded.",
      },
      {
        topic: "Canonical long flag completion",
        behavior:
          "Filtered to the current command and visible catalog entries.",
      },
      {
        topic: "Negative and one-character abbreviation completion",
        behavior: "Includes Bazel's negative -x- spelling.",
      },
      {
        topic: "Old-name completion",
        behavior:
          "Old names resolve for hover/diagnostics but are not suggested.",
      },
      {
        topic: "common flag union",
        behavior: "Known in any reported non-startup command.",
      },
      {
        topic: "always safe flag intersection",
        behavior: "Known in every reported non-startup command.",
      },
      {
        topic: "Native flag hover",
        behavior:
          "Docs, spellings, type, default, enums, scopes, tags, and expansions.",
      },
      {
        topic: "Required-value and negative-boolean checks",
        behavior: "Only contradictions proven by the exact catalog are errors.",
      },
      {
        topic: "Scope, old-name, deprecation, and status checks",
        behavior: "Severity follows the strength of catalog evidence.",
      },
      {
        topic: "Starlark build-setting flag exemption",
        behavior: "--//, --@, --no//, and --no@ bypass native lookup.",
      },
      {
        topic: "Internal flags omitted by Bazel",
        behavior: "flags-as-proto does not expose INTERNAL options.",
      },
      {
        topic: "Flag aliases and external rc aliases",
        behavior:
          "Not resolved because they are outside the native catalog and workspace snapshot.",
      },
      {
        topic: "Enum value completion",
        behavior: "Only exact nonempty enum sets reported by the 8.7 catalog.",
      },
      {
        topic: "Enum membership validation",
        behavior:
          "ASCII-case-insensitive, matching Bazel's converter, and only for an exact reported enum set.",
      },
      {
        topic: "Other flag-value completion",
        behavior:
          "Returns no values because non-enum converters do not expose a finite set.",
      },
      {
        topic: "Converter-specific value validation",
        behavior: "The server does not execute Bazel option converters.",
      },
    ],
  },
  {
    area: "Language server",
    rows: [
      {
        topic: "Command and directive completion",
        behavior: "Available without Bazel.",
      },
      {
        topic: "Import-path completion",
        behavior:
          "At most 512 eligible matches from a 131,072-path snapshot; ignored, metadata, output, symlink, non-regular, and non-UTF-8 paths are excluded.",
      },
      {
        topic: "Import document links",
        behavior: "Only active imports whose targets loaded successfully.",
      },
      {
        topic: "Import go-to-definition",
        behavior: "Only active imports whose targets loaded successfully.",
      },
      {
        topic: "Semantic tokens",
        behavior:
          "Directives/keys, conditions, paths, option tokens, and comments.",
      },
      {
        topic: "Separate flag-value semantic tokens",
        behavior:
          "A value token uses the same property category as its option.",
      },
      {
        topic: "Continuation and comment-run folding",
        behavior: "Catalog-independent structural ranges.",
      },
      {
        topic: "Token, logical-line, and file selection ranges",
        behavior: "Catalog-independent structural ranges.",
      },
      {
        topic: "Current-buffer syntax diagnostics",
        behavior: "Published on each document change.",
      },
      {
        topic: "Saved import-graph diagnostics",
        behavior:
          "Shown only while the open text still matches the indexed file.",
      },
      {
        topic: "Missing configuration warnings",
        behavior:
          "Qualified to the published workspace graph; never called invalid.",
      },
      {
        topic: "Command/config/import hover",
        behavior: "Structural facts are available without a flag catalog.",
      },
      {
        topic: "Document and workspace symbols for configurations",
        behavior: "Declarations use decoded names and exact name-only ranges.",
      },
      {
        topic: "Configuration references and highlights",
        behavior:
          "Case-sensitive decoded identity across saved and open graph files.",
      },
      {
        topic: "Rename",
        behavior:
          "Declared names only; nonempty bare fragments; collisions are refused.",
      },
      {
        topic: "Formatting",
        behavior:
          "Returns no edits because Bazel defines no canonical semantics-safe layout.",
      },
      {
        topic: "Implementation, code lens, and inlay hints",
        behavior: "Bazelrc handlers return no results.",
      },
    ],
  },
];
