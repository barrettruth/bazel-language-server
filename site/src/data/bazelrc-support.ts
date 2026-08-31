export const bazelrcSupportReviewed = "2026-08-31";
export const bazelrcSourceCommit = "a6d8d66737d56766c0f5377c62ccf2ee562a1a47";

export type BazelrcSupportRow = {
  feature: string;
  supported: boolean;
  boundary: string;
};

export type BazelrcSupportSection = {
  area: string;
  rows: BazelrcSupportRow[];
};

export const bazelrcSupport: BazelrcSupportSection[] = [
  {
    area: "Version and files",
    rows: [
      {
        feature: "Upstream Bazelrc language",
        supported: true,
        boundary: "Pinned to upstream Bazel 8.7.0.",
      },
      {
        feature: "Native flag catalog",
        supported: true,
        boundary:
          "Only a configured executable reporting numeric major 8 and minor 7.",
      },
      {
        feature: "Structural editing with another Bazel version",
        supported: true,
        boundary: "Still uses the upstream 8.7.0 grammar; no flag catalog.",
      },
      {
        feature: "Nearest-version or bundled catalog fallback",
        supported: false,
        boundary: "No catalog is safer than metadata from a different release.",
      },
      {
        feature: "Vendor-only grammar and configuration semantics",
        supported: false,
        boundary:
          "A vendor-suffixed 8.7 executable may supply flags, not a new language contract.",
      },
      {
        feature: ".bazelrc and *.bazelrc classification",
        supported: true,
        boundary: "The filename must be exact or end in .bazelrc.",
      },
      {
        feature: "Other rc filenames such as bazel.rc",
        supported: false,
        boundary: "Rename or configure the file with a .bazelrc suffix.",
      },
      {
        feature: "Starlark parsing, buildifier formatting, or lint",
        supported: false,
        boundary: "Bazelrc is an independent native-client language.",
      },
    ],
  },
  {
    area: "Logical grammar",
    rows: [
      {
        feature: "LF and CRLF continuation deletion",
        supported: true,
        boundary:
          "Deleted globally before line splitting, including in quotes and comments.",
      },
      {
        feature: "Native whitespace delimiters",
        supported: true,
        boundary: "Space, TAB, CR, and LF delimit tokens.",
      },
      {
        feature: "Mid-token comments",
        supported: true,
        boundary: "An unescaped, unquoted # ends the logical line.",
      },
      {
        feature: "Single/double quotes and fragment concatenation",
        supported: true,
        boundary: "Quote bytes are removed; adjacent fragments form one token.",
      },
      {
        feature: "Backslash byte escaping",
        supported: true,
        boundary: "Inside and outside quotes; no shell expansion is performed.",
      },
      {
        feature: "Unclosed quotes and dangling backslashes",
        supported: true,
        boundary: "Accepted as Bazel's native tokenizer accepts them.",
      },
      {
        feature: "Empty quoted tokens",
        supported: true,
        boundary: "Discarded rather than retained as empty arguments.",
      },
      {
        feature: "Physical ranges across continuations",
        supported: true,
        boundary:
          "Editor ranges map logical tokens back to the original UTF-8 bytes.",
      },
      {
        feature: "Argless ordinary entries",
        supported: false,
        boundary:
          "Bazel 8.7 discards them; they declare no empty configuration.",
      },
    ],
  },
  {
    area: "Imports and discovery",
    rows: [
      {
        feature: "import PATH",
        supported: true,
        boundary: "Exact arity; read failures are errors.",
      },
      {
        feature: "try-import PATH",
        supported: true,
        boundary: "Exact arity; read failures are quiet.",
      },
      {
        feature: "try-import-if-bazel-version CONDITION PATH",
        supported: true,
        boundary: "Bazel 8.7 SemVer grammar, evaluated against baseline 8.7.0.",
      },
      {
        feature: "Configured vendor label in conditional imports",
        supported: false,
        boundary: "Conditions do not use suffixes such as 8.7.0-imc2.",
      },
      {
        feature: "Depth-first textual import expansion",
        supported: true,
        boundary: "No later-version import-depth cap.",
      },
      {
        feature: "Diamond import replay",
        supported: true,
        boundary: "Entries replay and the repeated import receives a warning.",
      },
      {
        feature: "Active import cycle detection",
        supported: true,
        boundary: "Reported as an error.",
      },
      {
        feature: "Exact %workspace%/ substitution",
        supported: true,
        boundary: "Other %workspace% spellings are literal paths.",
      },
      {
        feature: "Absolute and workspace-root-relative paths",
        supported: true,
        boundary: "Relative paths use Bazel's workspace process directory.",
      },
      {
        feature: "Containing-file-relative imports",
        supported: false,
        boundary: "That common editor convention does not match Bazel 8.7.",
      },
      {
        feature: "Environment-variable or tilde expansion",
        supported: false,
        boundary: "$VAR, ${VAR}, and ~ remain literal.",
      },
      {
        feature: "Workspace .bazelrc graph",
        supported: true,
        boundary: "The sole graph root; all reachable imports are indexed.",
      },
      {
        feature: "System, home, and explicit --bazelrc layers",
        supported: false,
        boundary: "Not reconstructed by the language server.",
      },
      {
        feature: "Bazel 9.0 BAZELRC environment list",
        supported: false,
        boundary: "Outside the 8.7 contract.",
      },
      {
        feature: "Unsaved import graph traversal",
        supported: false,
        boundary:
          "Import topology updates after the file is saved and watched.",
      },
    ],
  },
  {
    area: "Commands and configs",
    rows: [
      {
        feature: "Bazel 8.7 commands and rc scopes",
        supported: true,
        boundary: "Static exact list, including always, common, and startup.",
      },
      {
        feature: "Command inheritance",
        supported: true,
        boundary:
          "Build/test/coverage and other 8.7 inheritance is transitive.",
      },
      {
        feature: "Unknown command sections",
        supported: false,
        boundary:
          "Warned; excluded from config indexing and flag completion/checks. Native hover may still identify a spelling.",
      },
      {
        feature: "Named configuration declarations",
        supported: true,
        boundary: "Recognized, non-startup base plus at least one option.",
      },
      {
        feature: "startup:name configurations",
        supported: false,
        boundary: "Startup parsing cannot consume command --config.",
      },
      {
        feature: "Top-level --config=name and --config name",
        supported: true,
        boundary: "Both spellings create configuration references.",
      },
      {
        feature: "Nested --config=name",
        supported: true,
        boundary:
          "Included in completion, navigation, and absence diagnostics.",
      },
      {
        feature: "Nested split --config name",
        supported: false,
        boundary: "Bazel 8.7 rejects it; the server reports an error.",
      },
      {
        feature: "Applicable configuration completion",
        supported: true,
        boundary:
          "Follows command inheritance and all declarations in the graph.",
      },
      {
        feature: "Open-buffer declaration overlays",
        supported: true,
        boundary: "An open file replaces declarations from its saved path.",
      },
      {
        feature: "Configuration go-to-definition",
        supported: true,
        boundary: "Returns every applicable saved or open declaration.",
      },
      {
        feature: "Effective option/config expansion",
        supported: false,
        boundary:
          "The server indexes references; it does not build a final command line.",
      },
      {
        feature: "Configuration expansion-cycle diagnostics",
        supported: false,
        boundary: "Requires ordered owner-to-reference expansion edges.",
      },
      {
        feature: "Repeated/deep expansion warnings",
        supported: false,
        boundary: "Requires the same expansion graph.",
      },
      {
        feature: "Automatic platform configuration selection",
        supported: false,
        boundary: "No --enable_platform_specific_config evaluation.",
      },
    ],
  },
  {
    area: "Native flags",
    rows: [
      {
        feature: "help flags-as-proto acquisition",
        supported: true,
        boundary: "Runs in the Bazel actor with all rc files ignored.",
      },
      {
        feature: "Bazel 8.7 proto fields 1–16",
        supported: true,
        boundary:
          "All reported documentation and semantic metadata are decoded.",
      },
      {
        feature: "Canonical long flag completion",
        supported: true,
        boundary:
          "Filtered to the current command and visible catalog entries.",
      },
      {
        feature: "Negative and one-character abbreviation completion",
        supported: true,
        boundary: "Includes Bazel's negative -x- spelling.",
      },
      {
        feature: "Old-name completion",
        supported: false,
        boundary:
          "Old names resolve for hover/diagnostics but are not suggested.",
      },
      {
        feature: "common flag union",
        supported: true,
        boundary: "Known in any reported non-startup command.",
      },
      {
        feature: "always safe flag intersection",
        supported: true,
        boundary: "Known in every reported non-startup command.",
      },
      {
        feature: "Native flag hover",
        supported: true,
        boundary:
          "Docs, spellings, type, default, enums, scopes, tags, and expansions.",
      },
      {
        feature: "Required-value and negative-boolean checks",
        supported: true,
        boundary: "Only contradictions proven by the exact catalog are errors.",
      },
      {
        feature: "Scope, old-name, deprecation, and status checks",
        supported: true,
        boundary: "Severity follows the strength of catalog evidence.",
      },
      {
        feature: "Starlark build-setting flag exemption",
        supported: true,
        boundary: "--//, --@, --no//, and --no@ bypass native lookup.",
      },
      {
        feature: "Internal flags omitted by Bazel",
        supported: false,
        boundary: "flags-as-proto does not expose INTERNAL options.",
      },
      {
        feature: "Flag aliases and external rc aliases",
        supported: false,
        boundary: "They are outside the native catalog and workspace snapshot.",
      },
      {
        feature: "Flag or enum value completion",
        supported: false,
        boundary: "Enum metadata appears in hover only.",
      },
      {
        feature: "Converter-specific value validation",
        supported: false,
        boundary: "The server does not execute Bazel option converters.",
      },
    ],
  },
  {
    area: "Language server",
    rows: [
      {
        feature: "Command and directive completion",
        supported: true,
        boundary: "Available without Bazel.",
      },
      {
        feature: "Import-path completion",
        supported: false,
        boundary: "Paths are not scanned from request handlers.",
      },
      {
        feature: "Import document links",
        supported: true,
        boundary: "Only active imports whose targets loaded successfully.",
      },
      {
        feature: "Import go-to-definition",
        supported: true,
        boundary: "Only active imports whose targets loaded successfully.",
      },
      {
        feature: "Semantic tokens",
        supported: true,
        boundary:
          "Directives/keys, conditions, paths, option tokens, and comments.",
      },
      {
        feature: "Separate flag-value semantic tokens",
        supported: false,
        boundary:
          "A value token uses the same property category as its option.",
      },
      {
        feature: "Continuation and comment-run folding",
        supported: true,
        boundary: "Catalog-independent structural ranges.",
      },
      {
        feature: "Token, logical-line, and file selection ranges",
        supported: true,
        boundary: "Catalog-independent structural ranges.",
      },
      {
        feature: "Current-buffer syntax diagnostics",
        supported: true,
        boundary: "Published on each document change.",
      },
      {
        feature: "Saved import-graph diagnostics",
        supported: true,
        boundary:
          "Shown only while the open text still matches the indexed file.",
      },
      {
        feature: "Missing configuration warnings",
        supported: true,
        boundary:
          "Qualified to the published workspace graph; never called invalid.",
      },
      {
        feature: "Command/config/import hover",
        supported: false,
        boundary: "Hover is native-flag-only.",
      },
      {
        feature: "Document and workspace symbols for configurations",
        supported: false,
        boundary: "Bazelrc requests do not expose symbols.",
      },
      {
        feature: "Configuration references and highlights",
        supported: false,
        boundary: "Only go-to-definition is implemented.",
      },
      {
        feature: "Rename",
        supported: false,
        boundary: "No Bazelrc rename or prepare-rename.",
      },
      {
        feature: "Formatting",
        supported: false,
        boundary: "Bazelrc never reaches buildifier.",
      },
      {
        feature: "Implementation, code lens, and inlay hints",
        supported: false,
        boundary: "Bazelrc handlers return no results.",
      },
    ],
  },
];
