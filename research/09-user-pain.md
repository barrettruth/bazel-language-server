# 09 — User pain: evidence on editing Bazel build files

All figures gathered **2026-08-25**. Every claim below has a URL + date. Where I could
not get data, I say so rather than guess.

## 0. Method, and what I could not get

Gathered: GitHub REST/GraphQL via authenticated `gh` (starpls, vscode-bazel, bazel,
bazel-lsp, starlark-lsp, hirschgarten, SIG-rules-authors), HN Algolia API (full comment
trees, keyword-filtered), VS Code Marketplace `extensionquery` API, Open VSX API,
JetBrains Marketplace API, Zed extension API, `packagecontrol.io`, MELPA download
counts, raw `nvim-lspconfig` / `helix` / `mason-registry` config files, GitHub release
asset download counts, two peer-reviewed papers (PDF text extracted locally),
Bazel/EngFlow/Databricks/Mercari/Spotify engineering blogs.

**Gaps:**
- **Reddit is hard-blocked from this network** (403 "blocked by network security" on
  `reddit.com`, `old.reddit.com`, `api.reddit.com`, and via `r.jina.ai`; the search tool
  returned zero results for every reddit-scoped query). No r/bazel, r/ExperiencedDevs or
  r/golang evidence in this document. This is a real hole — r/bazel is the one place I'd
  expect casual "how do I get autocomplete" traffic.
- **Bazel Slack** (`slack.bazel.build`) has no public archive. One vscode-bazel issue
  links into it (`bazelbuild.slack.com/archives/CA31HN1T3/p1766516123474319`, cited in
  #504) but the content is not retrievable.
- **The 2025 Bazel User Survey raw results** are a Google Slides deck
  (`docs.google.com/presentation/d/1oy9vXLrgrTG_r7yMk5TgYjdo0PSRn2ccSk0gyatVnJM`) linked
  from bazelbuild/bazel discussion #25659 (2025-03-21); not machine-readable here. The
  2024 survey summary *is* public (§1.6).
- `hn.algolia.com` and `reddit.com` fail TLS verification under this network's MITM proxy
  with the nix-provided `curl`; I used `/usr/bin/curl` (macOS keychain) for those hosts.

---

## 1. Primary evidence

### 1.1 The single loudest signal: vscode-bazel#1, open 8 years

`bazel-contrib/vscode-bazel#1` — "Implement language server for Starlark"
<https://github.com/bazel-contrib/vscode-bazel/issues/1>
Opened **2018-09-11**. **71 reactions (56 👍, 15 ❤️), 56 comments. Still open.**
It is the most-reacted issue in a repo with 242 issues; second place (#179, source-code
autocomplete, out of our scope) has 32.

Note the repo moved: `bazelbuild/vscode-bazel` now 301-redirects to
`bazel-contrib/vscode-bazel` (GitHub search rejects the old name).

Timeline of that thread, which is the history of this whole problem:

- **laurentlb (Bazel/Starlark TL at Google), 2018-09-12** (11 reactions):
  > "Inside Google, we have an implementation of LSP in progress (with completion, jump
  > to definition, and diagnostics). The bad news is that it relies on an internal
  > framework. I expect it to be open-sourced early 2019, but I have little control on
  > that."
- **laurentlb, 2020-01-07:**
  > "No, sorry. There's some code at Google that I hoped would get open-sourced. It's
  > unfortunately not happening."
- **josiahsrc, 2021-01-29** (20 reactions): BYU capstone team builds `BYU-Bazel/bazel-ls`
  in Java. **2022-02-10:** "we had to drop it due to 1) our capstone course ending and
  2) there not being enough support behind it."
- **withered-magic, 2024-01-17** (9 reactions): announces starpls.
- **cameron-martin, 2024-01-18** (4 reactions): announces bazel-lsp fork of starlark-rust.
- **withered-magic, 2024-04-01:** documents that `bazel info build-language` "doesn't
  contain any documentation strings for any of the builtin rules or their attributes",
  and that `MODULE.bazel`/`WORKSPACE` builtins can't be sourced from ApiExporter at all,
  so he **hand-copied the docs from the Build Encyclopedia** into
  `crates/starpls_bazel/data`. (Still true in the checkout at
  `upstream/starpls/crates/starpls_bazel/data`.)
- **chrisdothtml, 2025-04-17:**
  > "Just checking in as this is still relevant in 2025, and makes working in
  > large/complex bazel repos (especially monorepos) very difficult (certainly for
  > people not intimately familiar with bazel)"
- **keith (Lyft principal eng, Bazel iOS maintainer), 2025-04-17:** "fwiw starpls works
  very well, we should probably close this issue as i don't think it would make sense to
  have a competing project in this repo unless there's some amount of pre-installing we
  can do so that folks get that 'for free'"
- **guw (Salesforce), 2025-04-18** (2 reactions):
  > "As a user I expect to install the extension from the marketplace and things work.
  > Thus, I think that the actual work is to ensure a LS is bundled and started with the
  > extension."
- **ivhacks, 2026-01-19:** "💯💯💯💯💯💯💯💯💯💯"
- **luispadron, 2026-01-20:** "+1 to using `starpls` (its great!) and bundling it with
  the extension so it Just Works. No competing LSP please..."
- **brentleyjones, 2026-02-09:** "Can we default to using/bundling `starpls`?"
- **cbandera (current vscode-bazel maintainer), 2026-02-09:** "I support the idea…
  However I am not familiar with the history of why there are 2 different language
  servers and why they were only recommended as an alternative in the first place. Also
  I haven't checked up on how they differ in features, stars and activity at the moment."
- **cameron-martin, 2026-03-24:** "it seems like it's time to pick one and automatically
  install it with the extension."

**And it still hasn't happened.** In `upstream/vscode-bazel` at v0.14.0 (released
2026-03-31), `package.json:215` still declares `"bazel.lsp.command": { "default": "" }`
with the description "The executable to launch for the LSP (experimental)", and
`README.md:75-82` says:

> "There are currently two compatible language servers: bazel-lsp … starpls …
> **We can't currently make any recommendation between these two.**"

So the officially-blessed VS Code extension, with ~975k installs, ships **no** language
server, eight years after the request.

### 1.2 What the shipped default actually does (and why it hurts)

Because there is no bundled LSP, vscode-bazel implements go-to-definition and completion
by shelling out to `bazel query`. From
`upstream/vscode-bazel/src/definition/bazel_goto_definition_provider.ts:46-50`:

```ts
const queryResult = await new BazelQuery(
  getBazelExecutablePath(),
  workingDirectory.fsPath,
).queryTargets(`kind(rule, "${targetName}") + kind(file, "${targetName}")`);
```

and `src/completion-provider/bazel_completion_provider.ts:150` — "Runs a bazel query
command to acquire labels of all the targets in the …".

The consequences are the top-reacted *recent* bug:

`bazel-contrib/vscode-bazel#504` — "Bug: hitting process limit due to `bazel query ...:*`
spawning from extension", **2026-01-02, 15 reactions**
<https://github.com/bazel-contrib/vscode-bazel/issues/504>

> **lgo, 2026-01-02:** "I'm able to consistently repro an issue where the extension
> spawns 1000s of `bazel query ...:*` … this would result in the following error … `fork
> failed: resource temporarily unavailable`"
> **lamcw, 2026-01-06:** "Even with the debouncing, on a large scale workspace the
> default `bazel query ...:*` is enough to consume all CPU resources on the Bazel host."
> **Silic0nS0ldier, 2026-01-06:** "if the issue reproduces you may find you've just
> DoS'd yourself and will need to reboot."

Fixed by #542, shipped in 0.13.2 (2026-01-24) — but only by debouncing, not by changing
the architecture. Related: `#490` "Allow to selectively enable/disable features of this
extension" (2025-12-11, 6 reactions): "This is either because of performance reasons
(long running queries in the background)…".

**This exact failure mode recurs across every Bazel-backed editor tool** — see §1.5
(starpls #407) and §5.3 (Databricks Metals v2). It is the single most reproducible
technical pain point in the corpus.

### 1.3 vscode-bazel: full recurring-complaint list

242 issues, 67 open. Ranked by reactions (open unless noted):

| # | Reactions | Date | Title |
|---|---|---|---|
| 1 | 71 | 2018-09-11 | Implement language server for Starlark |
| 179 | 32 | 2020-01-28 | Supporting Source Code Autocomplete *(out of scope — hedron_compile_commands)* |
| 186 | 16 | 2020-02-20 | Integrate with Test Explorer |
| 504 | 15 | 2026-01-02 | `bazel query` process explosion *(closed)* |
| 294 | 8 | 2023-01-31 | bzlmod support |
| 217 | 8 | 2020-09-16 | Generate build/debug targets in `launch.json` |
| 490 | 6 | 2025-12-11 | Selectively enable/disable features |
| 453 | 6 | 2025-06-17 | Support project view (`.bazelproject`) files |
| 73 | 7 | 2019-03-19 | Go-to-definition for bazel rules and macros *(closed)* |
| 353 | 3 | 2024-03-01 | Shortcut to jump to BUILD or BUILD.bazel file *(closed; shipped v0.13.0)* |

Recent bugs that are specifically about *editing Bazel files* and are still open:
- `#442` (2025-03-01) "Bazel 8 **Symbolic Macros** break CodeLens and Target Tree"
- `#622` (2026-04-21) "Go to Definition treats `go_proto_library.importpath` as Bazel
  label and runs invalid query"
- `#331` (2024-01-19) "`*.bzl` files not recognized by completion provider"
- `#416` (2024-10-14) "Handle external dependencies"
- `#428` (2025-01-03) "Extension cannot find bazel root without a WORKSPACE file"
- `#621` (2026-04-21) "getBazelWorkspaceFolder for multiple MODULE.bazel files"
- `#684` (2026-08-19) "VS Code ↔ Bazel workspace root divergence"
- `#435` (2025-02-03) "Bazelrc Language Server"

### 1.4 starpls: 187 issues, but nothing above **6** reactions

`withered-magic/starpls` — 217★, 32 forks, 57 open / 130 closed issues, **20 open PRs**.
Last commit **2025-12-03** (`ac25eca`, "Accept 1/0 as bool (#414)"). Last release
**v0.1.22, 2025-08-30**. As of 2026-08-25 that is **~9 months without a commit and
~12 months without a release.**

Top-reacted issues (max is 6 — an order of magnitude below vscode-bazel#1):

| # | R | State | Date | Title |
|---|---|---|---|---|
| 354 | 6 | open | 2024-12-10 | support **symbolic macros** |
| 379 | 6 | open | 2025-03-23 | Support custom stubs |
| 232 | 2 | open | 2024-04-25 | LSIF/SCIP support |
| 125 | 2 | open | 2024-03-30 | **Find references** |
| 303 | 2 | closed | 2024-11-11 | Enabling starpls in vscode-bazel **disables** target autocomplete in BUILD.bazel |
| 100 | 1 | open | 2024-03-27 | go to bazel rule(s) |
| 225 | 1 | open | 2024-04-23 | Support for multi-workspaces mode |
| 420 | 1 | open | 2026-01-23 | **Add Bazel 9 builtins** |

Open issues by theme:
- **Bazel-version lag:** #420 Bazel 9 builtins (2026-01-23); #354/#388 symbolic macros
  (Bazel 8 feature, filed 2024-12-10, unimplemented); #410 unknown kwarg
  `applicable_licenses` (2025-11-25).
- **Bazel-invocation fragility:** #418 "`bazel info` failed during initialization，`at
  most one key may be specified`" (2026-01-09); #371 "Stack Overflow fetching builtin
  rules" (2025-01-09); #310 "recover if initial `bazel info` calls fail"; #399 —
  breaks behind Aspect CLI / shell wrappers (per maintainer, 2025-11-06).
- **Missing LSP surface:** #267 formatting; #125 find references; #43 inlay hints;
  #265/#266 auto-import; #232 LSIF/SCIP; #376 Starlark debug protocol.
- **Scale/lock contention:** #407 (below).

### 1.5 starpls: exactly what it implements (from source)

`upstream/starpls/crates/starpls/src/event_loop.rs:211-221` — the entire request
dispatcher:

```rust
.on::<extensions::ShowSyntaxTree>(requests::show_syntax_tree)
.on::<extensions::ShowHir>(requests::show_hir)
.on::<lsp_types::request::Completion>(requests::completion)
.on::<lsp_types::request::DocumentSymbolRequest>(requests::document_symbols)
.on::<lsp_types::request::GotoDefinition>(requests::goto_definition)
.on::<lsp_types::request::GotoDeclaration>(requests::goto_declaration)
.on::<lsp_types::request::HoverRequest>(requests::hover)
.on::<lsp_types::request::References>(requests::find_references)
.on::<lsp_types::request::SignatureHelpRequest>(requests::signature_help)
```

Seven LSP methods. **Not implemented at all:** `textDocument/formatting`,
`textDocument/rename`, `textDocument/codeAction`, `textDocument/inlayHint`,
`textDocument/semanticTokens`, `textDocument/codeLens`, `workspace/symbol`,
`textDocument/documentHighlight`, `textDocument/foldingRange`, `callHierarchy`.

And `find_references` is **not** what a Bazel user wants. From
`crates/starpls_ide/src/find_references.rs:28-31`:

```rust
let finder = Finder::new(name.as_str());
let offsets = finder
    .find_iter(self.file.contents(self.sema.db).as_bytes())
```

It scans **one file** (`self.file`) and matches only Starlark `ast::Name` / `ast::NameRef`
nodes — i.e. local variables and function names. **There is no "find all references to
`//foo:bar`" and no "rename this target and rewrite every referring label".**

Label completion is behind `--experimental_enable_label_completions`
(`crates/starpls/src/commands/server.rs:27-32`, `default_value_t = false`), and it has
the same lock-contention bug as vscode-bazel:

`starpls#407`, **2025-09-16**, filed by **olafurpg** (Ólafur Páll Geirsson — author of
Scalafmt/Metals, now at Databricks):
> "We have `--experimental_enable_label_completions` enabled for all users in a large
> monorepo and I've observed that whenever I edit BUILD files, the message 'Refreshing
> all workspace targets' appears in the VS Code status bar and **this process appears to
> hold onto a Bazel lock**. … A workaround is to disable the experimental flag for now."

Open, unanswered, 11 months later.

### 1.6 The maintainer bottleneck is the real starpls problem

20 open PRs. Authors include **fmeum** (Fabian Meumertzheim, Bazel core),
**keith** (Keith Smiley, Lyft), **sluongng** (Son Luong Ngoc), **PeterCardenas**,
**Ahajha**, **steeve**, **cerisier**. Newest: 2026-07-19. Last *merge*: **2025-12-03**.

The clearest case is **formatting**, requested in `#267` (2024-06-30):

- **PR #401** "feat: implement textDocument/formatting" — steeve, **2025-08-11**,
  +112/−4, 5 files, spawns `buildifier`.
- steeve, 2025-08-20: "Gentle ping @withered-magic :)"
- joelreymont on #267, 2025-08-18: "Any ETA for merging this? I'd love to use it with
  Helix!"
- withered-magic, **2025-12-19**: "sorry for late movement here, I'll try to get this in
  soon!"
- steeve, **2026-01-14**: "gentle ping !"
- withered-magic, 2026-01-14: "Can we gate this behind a flag … Otherwise LGTM!"
- steeve, 2026-01-20: "sure!"; withered-magic: "lmk if you'd like me to make that change
  too and then I can just merge"
- **cerisier, 2026-04-15**: a *third* contributor picks it up and does the gating.
- **Still open, 2026-08-25.** One year and two weeks.

Helix users genuinely have no BUILD-file formatting as a result: in
`helix-editor/helix` `languages.toml`, `formatter = {command = "buildifier"}` (line 5456)
is attached to the **`tilt`** language, not `starlark`. The `starlark` entry (lines
2946-2965) has `language-servers = [ "starpls", "buck2" ]` and no formatter.

### 1.7 The upstream Bazel blocker: `builtin.proto` has been stalled for 2+ years

Every Bazel-aware Starlark LSP needs a machine-readable description of the build
language. The state of that plumbing:

- **`bazelbuild/bazel` PR #21929** — fmeum, "Include Bzlmod globals in `builtin.proto`",
  opened **2024-04-08**, body: "Work towards bazelbuild/vscode-bazel#1".
  **Still open. `mergeable_state: dirty`. Last touched 2026-07-03.**
  The plea comment from vogelsgesang (2024-06-07) has **21 reactions** — more than any
  starpls issue:
  > "I am not sure why there was little visible progress over the last 2 months, but
  > maybe it would help make progress, if the Bazel project becomes more aware of the
  > community demand for auto-completion inside VSCode / other LSP-based editors."
- **`bazelbuild/bazel` #21979** (tetromino, Google) — "Stardoc protos … should export
  parameter type annotations for builtin functions", opened **2024-04-11**,
  **13 reactions**, still open. Last comment, **ofek, 2026-02-28**: *"Has there been any
  progress on this?"* — no reply.
- **`bazelbuild/bazel` #15817** — "Add Documentation to Rules Emitted by `bazel info
  build-language`" (2022-07-06). **Closed 2025-09-26 as `not_planned` by the stale bot**
  after two stale warnings and no human action. This is why hovering a `cc_library`
  attribute has no docs unless the server ships hand-copied text.
- **`bazelbuild/bazel` #7163** — "Allow rule sets to contribute suggested fix for
  undefined starlark symbol" (2019-01-17), still open.
- There is **no `area-IDE` label** on bazelbuild/bazel (search returns 0).

### 1.8 Bazel's own documentation does not know starpls exists

<https://bazel.build/install/ide> as fetched **2026-08-25**. Its VS Code section lists
exactly two features:

> * Bazel Build Targets tree
> * Starlark debugger for `.bzl` files during a build

No mention of starpls, bazel-lsp, `bazel.lsp.command`, or any Starlark language server
anywhere on the page. It still links to **Atom** (`atom.io/packages/language-bazel`;
Atom was sunset 2022-12-15) and to **`bazelbuild/vim-bazel`**, which is **archived, last
pushed 2022-04-09**. "Building your own IDE plugin" points at a **2016** blog post.

### 1.9 Official Bazel survey: IDE integration is a named problem

Bazel Q2 2024 Community Update, **2024-07-22**
<https://blog.bazel.build/2024/07/22/bazel-q2-2024-community-update.html> — four key
takeaways from the Q1 2024 developer satisfaction survey, one of which is:

> "**Challenges with IDE integrations** and the transition to Bzlmod."

(Another is "Strong satisfaction with … Starlark" — the *language* is fine; the tooling
around it isn't.)

### 1.10 Hacker News, 2023–2026

Threads mined in full via the Algolia item API and keyword-filtered.

**"The next generation of Bazel builds"** — 2025-04-06, 89 pts, 88 comments,
<https://news.ycombinator.com/item?id=43601356>

- **throwaway127482, 2025-04-10** — the most direct criticism of starpls I found
  anywhere <https://news.ycombinator.com/item?id=43643353>:
  > "In the next generation of build tools, I really wish Starlark could be replaced with
  > some subset of TypeScript, which would have drastically better IDE support. …
  > **Starlark is super difficult to read, navigate, and write**, and there are a lot of
  > performance gotchas. **I know there are efforts like starpls (language server for
  > Starlark) but in my experience it really falls short.**"
- **jen20, 2025-04-10** <https://news.ycombinator.com/item?id=43645382>:
  > "The biggest problem with everything NotBazel in this space is IDE support."
- **mook, 2025-04-10** <https://news.ycombinator.com/item?id=43646141>:
  > "the part I hated most about Bazel was the fact that there were in fact **two
  > dialects; one for BUILD files and one for .bzl files**. The things you do in them are
  > different but the documentation is always vague on what goes where."

**"BazelCon 2024 Recap"** — 2024-10-23, 53 pts, 39 comments,
<https://news.ycombinator.com/item?id=41925622>

- **johnfr2, 2024-10-27** <https://news.ycombinator.com/item?id=41962522>:
  > "I worked in a huge big tech with infinite resources. They say monorepo using bazel
  > and gazelle is a success there, I personally found it the worst dev experience ever…
  > Everything was so slow, **IDE doesn't work properly, there was no easy debugger,
  > intellisence, refactoring, Code generation**… Now every enterprise which calls me on
  > LinkedIn, I ask if there is usage of monorepo, if it has, I just answer that I am not
  > interested…"
- **ninjazee124, 2024-10-23** <https://news.ycombinator.com/item?id=41928840>:
  > "We tried Bazel for a java monorepo. **Not having good IDE support was a deal-breaker**
  > because it would complicate things for onboarding and junior devs"
- **anothername12, 2024-10-23** <https://news.ycombinator.com/item?id=41928487>:
  > "It seems to replace the go mod stuff so we have something called gazelle that figured
  > it out and third party ide plug-ins. **The plug in for IntelliJ is janky.**"
- **sebastos, 2024-10-23** <https://news.ycombinator.com/item?id=41929970>:
  > "So it's 2024, and the plan is that we're going to wait another few years for BSP to
  > be finished before vscode support is anything more than a toy? If their goal is
  > adoption, that is just _awful_ pathfinding."
- **fire_lake, 2024-10-23:** "I found maintaining IDE files and a Bazel build side by
  side to be surprisingly easy. **It's for sure a weak point with Bazel though.**"

**"Ask HN: Anyone using Bazel at startups?"** — 2023-12-01, 36 comments,
<https://news.ycombinator.com/item?id=38493940>

- **shpx, 2023-12-02** <https://news.ycombinator.com/item?id=38496904> — the clearest
  articulation of the core BUILD-editing loop pain in the entire corpus:
  > "**Every import you write you have to write in two files** or three if you have to
  > export it. It's double the typing work and **finding the target is a pain because
  > there's no jump to definition of the Bazel target so it's even more typing to find
  > it.**"
- **AlexITC, 2023-12-03** <https://news.ycombinator.com/item?id=38511054>:
  > "IDE support was poor, in theory, we could just use intellij but most of the times it
  > required help from the bazel-guy, in my case, I got frustrated about the needed
  > maintenance and just accepted that an IDE wouldn't work. Given the lack of IDE, it
  > means that we couldn't easily execute a specific test from the UI, having to keep
  > notes for the cli."

**"Build files are the best tool to represent software architecture"** (jmmv/Julio
Merino) — 2025-10-03, 50 pts, 43 comments,
<https://news.ycombinator.com/item?id=45467751>

- **jmmv, 2025-10-07** <https://news.ycombinator.com/item?id=45506759>:
  > "In one codebase I have to deal with, the Bazel build has ~10k targets whereas the
  > previous non-Bazel build had ~400. … **The build files are unreadable. If targets
  > don't mean anything to a human, updates to build files become pure toil** (and is
  > when devs ask for build files to be auto-generated from source). **IDE integrations
  > (particularly via the IntelliJ Bazel plugin) become slower** because generating
  > metadata for those targets takes time."
- **bloppe, 2025-10-08** <https://news.ycombinator.com/item?id=45516520> — note the
  *mental model*, even though the tool name is wrong (it's `build_cleaner`, not
  `buildifier`):
  > "At Google, you can just run 'buildifier' to auto-generate the build files from the
  > source code import statements, so clearly the build files are redundant."

**Misc:**
- **__float, 2024-12-29** ("So you want to write Java in Neovim"),
  <https://news.ycombinator.com/item?id=42537502>: "**Bazel's lack of good IDE options
  definitely hampers its adoption.**"
- **HippoBaro, 2024-02-22** <https://news.ycombinator.com/item?id=39467376>: "JetBrain
  IDEs have a plug-in for Bazel that's incredibly slow and buggy. It's a shame because I
  used to really enjoy using their product, but now it's barely usable."
- **derriz, 2026-05-13** <https://news.ycombinator.com/item?id=48125783>: "Our bazel
  system is **full of custom skylark code** so understanding the build means effectively
  reading a bunch of ad-hoc code written with varying degrees of competence and with
  confusing dependencies. I'm kinda ashamed I don't have a deep understanding of a tool I
  use daily."

### 1.11 The Bazel community leadership's own view

`bazel-contrib/SIG-rules-authors#52` — "Starlark LSP", opened 2022-07-18, **closed
2025-10-29** <https://github.com/bazel-contrib/SIG-rules-authors/issues/52>

- **alexeagle (Aspect Build; long-time Bazel community lead), 2022-11-28:**
  > "FWIW at BazelCon this year I discussed with a few Googlers who are on the team that
  > would naturally own this. **They haven't even considered working on it**, and one told
  > me that the **Cider-only LSP that they do have may have been broken by the change to
  > the Monaco editor**. This is a hard thing to write, so I don't have any ideas for
  > getting this project off the ground."
- **alexeagle, 2025-10-29** (closing it):
  > "I think with starpls now available, we could consider this complete? **Though it
  > doesn't understand Bazel builtins.**"
- **withered-magic, 2025-11-06:** "starpls **should** understand Bazel builtins!"

That exchange is itself an evidence point: the person who runs the Bazel-adjacent
consultancy did not know what the leading Bazel LSP does. Discoverability is poor.

---

## 2. (a) Top 10 concrete editor-experience complaints, ranked

Ranked by (i) reaction/comment counts, (ii) independent recurrence across sources,
(iii) recency. Each has at least two independent sources.

**1. There is no language server installed by default — you must find, download, and
wire one up yourself.**
vscode-bazel#1 (71 reactions, 8 years, still open); guw 2025-04-18 "As a user I expect to
install the extension from the marketplace and things work"; brentleyjones 2026-02-09;
luispadron 2026-01-20; cbandera (maintainer) 2026-02-09 didn't know which to pick;
`bazel.lsp.command` default `""` in v0.14.0; bazel.build/install/ide (2026-08-25) doesn't
mention any LSP. Everything else on this list is downstream of this one.

**2. No jump-to-definition on a label, so you navigate BUILD files by grep.**
shpx 2023-12-02 ("there's no jump to definition of the Bazel target so it's even more
typing to find it"); vscode-bazel#73 (7 reactions, 2019); the shipped fallback spawns
`bazel query kind(rule,…)` per jump (`bazel_goto_definition_provider.ts:46`);
starpls#100 "go to bazel rule(s)" open since 2024-03-27; bazel-lsp#25/#30.

**3. Label/target autocomplete is either absent, off by default, or catastrophically
expensive.**
starpls's label completion is `--experimental_enable_label_completions`, default
**false**, and holds the Bazel lock on every keystroke-triggered refresh (#407,
olafurpg/Databricks, 2025-09-16, unanswered). vscode-bazel's alternative spawns
`bazel query ...:*` and DoS'd users' machines (#504, 15 reactions, 2026-01-02;
"enough to consume all CPU resources on the Bazel host", lamcw 2026-01-06). And enabling
starpls *removes* the extension's own completion: starpls#303 (2024-11-11) "Enabling
starpls in vscode-bazel disables target autocomplete in BUILD.bazel files."

**4. Anything that touches Bazel fights the Bazel lock / competes with the user's build.**
vscode-bazel#504; vscode-bazel#490 ("long running queries in the background");
starpls#407; starpls README warns "if your VSCode setup also has any tasks that run Bazel
commands on open, those might temporarily block the server from starting up because of
the Bazel lock"; Databricks Metals v2 (2026-08-11): "Metals v2 **never invokes Bazel via
the BSP server unless the user explicitly asks it to**. In a large monorepo, background
IDE sync can take the Bazel lock and compete with developer-initiated" builds.
Three independent teams hit the identical wall.

**5. Hover/completion has no rule or attribute documentation, because Bazel won't export
it.**
`bazel info build-language` emits no doc strings — withered-magic, 2024-04-01, on
vscode-bazel#1, who then hand-copied the Build Encyclopedia into
`crates/starpls_bazel/data`. bazel#15817 asked for this in 2022 and was **auto-closed as
`not_planned` on 2025-09-26**. bazel#21979 (13 reactions) and PR #21929 (21 reactions on
one comment) are both still open after 2+ years. Also affects IntelliJ (bazel#15817's
original screenshot shows the same generic placeholder for every builtin rule).

**6. No find-references and no rename for a target.**
starpls#125 "Find references" open since 2024-03-30; starpls's `find_references` is
single-file, Starlark-identifier-only (`find_references.rs:29-31`); no `rename` handler
in `event_loop.rs`. Renaming a target means grepping for `"//pkg:name"` and hoping. The
Google 2012 paper flags exactly this: "even simple changes like renaming a class or
moving it from one package to another can take several days or weeks when operating at
this scale."

**7. New Bazel language features arrive in the editor a year or more late.**
Symbolic macros landed in Bazel 8 (Dec 2024). starpls#354 "support symbolic macros"
(6 reactions) has been open since **2024-12-10**; #388 (2025-05-22) reports the resulting
false type error. vscode-bazel#442 "Bazel 8 Symbolic Macros break CodeLens and Target
Tree" open since 2025-03-01. starpls#420 "Add Bazel 9 builtins" opened 2026-01-23 —
Bazel 9.2.0 is current and starpls has not shipped a release since 2025-08-30.

**8. Formatting is not part of the language server; you bolt buildifier on separately.**
starpls#267 (2024-06-30) → PR #401 (2025-08-11) → **still unmerged 2026-08-25** after
three contributors and four pings. Helix `starlark` has no formatter at all
(`languages.toml:2946-2965`). vscode-bazel invokes buildifier itself, which then
double-formats or conflicts (vscode-bazel#483 `--lint=off` workaround, #487 MODULE.bazel
support, #429/#427 "buildifier not found even when executable path is set").

**9. The tooling breaks the moment your repo isn't a vanilla single-workspace layout.**
starpls#225 "Support for multi-workspaces mode" (2024-04-23); #399/#418 — starpls's
`bazel info` probe fails behind Aspect CLI or a shell wrapper (maintainer confirmed
2025-11-06); #231/#337/#343 Bazel prelude support (broke again in ≥v0.1.15);
vscode-bazel#428 "cannot find bazel root without a WORKSPACE file" (2025-01-03), #621
multiple MODULE.bazel, #684 "VS Code ↔ Bazel workspace root divergence" (2026-08-19),
#446 "'Failed to find a Bazel Workspace file' on non bazel repos that include BUILD
files".

**10. Two files to edit for one import; the BUILD file is toil you can't automate away
outside Google.**
shpx 2023-12-02 ("Every import you write you have to write in two files or three");
jmmv 2025-10-07 ("updates to build files become pure toil … is when devs ask for build
files to be auto-generated from source"); yegle 2024-06-16 "There are 3 tools that make
maintaining BUILD files enjoyable: buildifier, buildozer and **build_cleaner
(internal only unfortunately)**" <https://news.ycombinator.com/item?id=40695386>;
verible PR#1505 "There is a tool `build_cleaner` that we use within Google, but I think
it is not released anywhere :(".
**This is the highest-value complaint and no LSP currently addresses it.**

Honourable mentions that didn't make the top 10: `.bazelrc` has its own separate LSP
(vscode-bazel#435, salesforce-misc/bazelrc-lsp); no test-explorer integration
(vscode-bazel#186, 16 reactions); `.bazelproject`/project-view files unsupported outside
JetBrains (#453, 6 reactions).

---

## 3. (b) Which of these starpls already solves

Verified against `upstream/starpls` @ `ac25eca` (2025-12-03) and its README roadmap.

### Solved, and solved well
- **Starlark semantics in `.bzl`:** error-resilient parser, type inference, unbound
  variables, argument validation, `struct`/provider/`rule` attribute awareness,
  PEP-484 type comments (`--experimental_infer_ctx_attributes` gives `ctx.*` completion
  inside rule impls). This is genuinely the best-in-class piece and is what earned
  keith's "starpls works very well" (2025-04-17).
- **Bazel builtins from the live toolchain:** it shells `bazel info build-language` +
  bundles `builtin.proto`, so `cc_library` attributes complete. (alexeagle didn't know
  this as of 2025-10-29 — a marketing failure, not a technical one.)
- **`load()` resolution**, including into external repos and through bzlmod repo mapping
  (`bazel mod dump_repo_mapping`), and jumping through re-exported symbols (#395,
  merged 2025-06-29).
- **Goto-definition on labels/targets** — listed `[x]` in the README roadmap; #375
  "Go to Definition for unexists labels" merged 2025-07-06.
- **Document symbols including Bazel targets.**
- **Hover** for variable types, signatures, docs.
- **Signature help.**

### Partially solved
- **Label completion** — exists but `--experimental_enable_label_completions`, default
  off, and #407 (lock contention) is open and unanswered.
- **Diagnostics** — Starlark type/arity errors only. **No** unresolved-label diagnostic,
  **no** buildifier lints, **no** cycle detection, **no** missing-`load` detection
  (bazel#7163, 2019, is the upstream ask for the last one).
- **`MODULE.bazel`** — supported via hand-maintained docs in `crates/starpls_bazel/data`;
  gaps keep surfacing (#421 `git_override` fields, 2026-02-06; #429 lock file churn,
  2026-06-08).

### Not solved at all
| Complaint | starpls status |
|---|---|
| #1 bundled/zero-config install | Out of its control; vscode-bazel still ships `""` |
| #6 find references to a **target** | `find_references.rs` is single-file, identifier-only |
| #6 rename a target + rewrite labels | No `rename` handler |
| #8 formatting | PR #401 unmerged for 12 months |
| Code actions ("add this dep", "fix visibility") | No `codeAction` handler |
| Workspace symbols (all targets in repo) | No `workspace/symbol` handler |
| Inlay hints | #43, open since 2024-02-22 |
| Semantic tokens | Not implemented |
| Code lens (build/test this target) | Not implemented; vscode-bazel does it separately |
| #7 symbolic macros (Bazel 8) | #354, open 20 months |
| #7 Bazel 9 builtins | #420, open |
| #9 multi-workspace | #225, open |
| #9 wrapped Bazel CLI (Aspect CLI) | #399, open |
| `.bazelrc` / `.bazelignore` / BCR `source.json` | Entirely out of scope; separate server exists for bazelrc only |
| #10 dependency maintenance (build_cleaner-equivalent) | Not attempted by any OSS LSP |

**The gap that matters most:** starpls is an excellent *Starlark* language server that has
partially grown Bazel awareness. It is not a *Bazel build-graph* language server. Every
feature that requires knowing the whole repository's target graph — references, rename,
workspace symbols, add-missing-dep code actions, unresolved-label diagnostics — is
missing, and its one graph-dependent feature (label completion) is disabled by default
because the way it gets the graph (calling `bazel`) doesn't scale.

---

## 4. (c) How many people plausibly use each thing

All numbers pulled 2026-08-25 from the respective official APIs.

| Thing | Metric | Value | Last activity |
|---|---|---|---|
| **buildifier / buildtools** | GH release asset downloads, v8.5.1 alone | **3,726,272** | 2026-01-30 (release), 2026-08-24 (push) |
| | v8.2.1 | 3,231,954 | 2025-06-10 |
| | stars | 1,189 | |
| **vscode-bazel** (`BazelBuild.vscode-bazel`) | VS Marketplace **installs** | **975,109** | v0.14.0, 2026-03-31 |
| | VS Marketplace rating | 3.55 / 5 (11 ratings) | |
| | Open VSX downloads | 161,568 | |
| | GH stars | 294 | pushed 2026-08-24 |
| **JetBrains Bazel** (new, id 22977) | Marketplace downloads | **1,613,351** (rating 3.72) | GA 2025-07 |
| **JetBrains Bazel for IntelliJ** (legacy, id 8609) | downloads | 2,322,950 (rating 3.38) | deprecated 2025-07 |
| **Bazel for CLion** (id 9554) | downloads | 218,627 (rating 4.12) | |
| **Bazel for Android Studio** (id 9185, Google) | downloads | 69,063 | |
| **hirschgarten** (JetBrains source) | stars | 148 | pushed 2026-08-24, issues on YouTrack |
| **starpls** | GH release downloads, **all versions** | **321,879** | |
| | v0.1.22 (current) | **136,348** | 2025-08-30 |
| | stars / forks | 217 / 32 | last commit 2025-12-03 |
| | max reactions on any issue | **6** | |
| **bazel-lsp** | GH release downloads, all versions | **31,810** | |
| | v0.6.4 (current) | 30,359 | 2025-02-12 |
| | stars | 82 | last **human** commit 2025-07-13; since then renovate-bot only |
| **Zed `starlark` ext** (bundles starpls) | Zed downloads | **55,063** | v0.4.1, 2026-03-01 |
| **bazel-stack-vscode** | VS Marketplace installs | 136,356 | **dead: last push 2023-08-07** |
| **bazelrc-lsp** | GH release downloads | 10,420 | 2026-06-02 |
| **tilt-dev/starlark-lsp** | stars | 35 | 2025-08-28; 9 issues ever |
| **emacs `bazel`** (bazel-contrib/bazel.el) | MELPA downloads | 27,313 (+10,832 legacy `bazel-mode`) | 2026-08-20 |
| **Sublime `BazelSyntax`** | Package Control installs | **2,105** | last modified 2020-03-21 |
| **`bazelbuild/vim-bazel`** | stars | 155 | **archived, 2022-04-09** |
| **hzeller/bant** (OSS build_cleaner) | stars | 30 | 2026-08-22 |
| **bazel-gazelle** | stars | 1,412 | 2026-08-20 |
| any **Bazel MCP server** | max stars | **11** | — |

**Shipped-and-configurable editor integrations (verified from source files):**

- **neovim / nvim-lspconfig** — `lsp/starpls.lua` (`cmd = {'starpls'}`,
  `filetypes = {'bzl'}`, `root_markers = {WORKSPACE, WORKSPACE.bazel, MODULE.bazel}`),
  added **2024-04-11** (`ec122b1`, PR #3102). Also ships `bzl.lua` (stackb, dead),
  `starlark_rust.lua`, `bazelrc_lsp.lua`, `buck2.lua`. Neovim's own
  `runtime/lua/vim/filetype.lua` maps `bzl|bxl|bazel` extensions and `BUILD`, `WORKSPACE`,
  `WORKSPACE.bzlmod`, `BUCK` filenames to `ft=bzl`, so `MODULE.bazel`/`BUILD.bazel`/
  `REPO.bazel` are covered by the `.bazel` extension rule.
- **Helix** — `starpls` is the **default** LSP for `starlark`
  (`languages.toml:151` `starpls = {command = "starpls"}`, `:2964`
  `language-servers = ["starpls","buck2"]`). Zero config needed. No formatter configured.
- **Zed** — `zaucy/zed-starlark` v0.4.1 (33★, 2026-03-01) declares
  `[language_servers.starpls]`, `buck2-lsp`, `tilt`; `src/starpls.rs` auto-downloads the
  starpls binary. 55,063 downloads.
- **mason.nvim registry** — `starpls@v0.1.22`, `buildifier@v8.5.1`, `bazelrc-lsp@v0.2.6`.
  `bzl` is **deprecated since 2025-12-01**: *"Servers offline and homepage no longer
  available. Use starpls instead."* No `bazel-lsp` package.
- **VS Code** — nothing bundled; manual `bazel.lsp.command`.
- **Emacs / Sublime** — syntax + buildifier only; no LSP client config ships for starpls
  in either ecosystem that I could find.

**Interpretation.** The honest read: **~1M** people have the VS Code Bazel extension and
**~1.6M** downloads of the JetBrains plugin, but only on the order of **10⁵** starpls
binaries have ever been fetched, and that number is inflated by CI (starpls#400 documents
a `download_file` + `rules_multitool` pattern where every clean CI machine re-downloads
it, and mason/Docker/devcontainer setups do the same — `jensmetzner/devcontainer-bazel`
exists purely to install "Bazelisk, Buildifier, Buildozer and Starpls"). A defensible
estimate of *humans who have a Bazel-aware Starlark LSP running today* is **low tens of
thousands**, against ~1M who have the extension. Roughly **95% of VS Code Bazel users are
running with no language server.**

The reaction ratio says the same thing from the other direction: 71 reactions asking for
an LSP on the extension, versus a maximum of 6 on any issue in the LSP that exists.
People want it; they don't know it's there or can't be bothered to install it.

---

## 5. (d) Is the market real, or does everyone just grep?

### 5.1 For "real"

- 71 reactions / 56 comments / 8 years on vscode-bazel#1, with fresh +1s in Jan/Feb/Mar
  **2026** — the demand has not decayed.
- Bazel's own Q1-2024 user survey named "Challenges with IDE integrations" as one of four
  headline findings (blog, 2024-07-22).
- Every editor ecosystem has *already built the socket*: nvim-lspconfig, Helix (default!),
  Zed, mason all ship starpls configs. The distribution channel exists and is free.
- The demand is loud enough that JetBrains **hired for it**: "JetBrains | Tech Lead /
  Senior for Bazel plugin | REMOTE" posted to HN Who's Hiring **2025-08-11**
  <https://news.ycombinator.com/item?id=44868787>, "Our team is working on Bazel
  integration for all our IDEs… looking to make it the best build tool experience you can
  get in an IDE." A commercial vendor is staffing a team against this exact problem.
- People who tried Bazel cite IDE experience as a *reason they left or refused*:
  ninjazee124 ("deal-breaker"), johnfr2 ("I just answer that I am not interested"),
  __float ("definitely hampers its adoption"), AlexITC.
- The incumbent has an obvious, quantified quality gap: 3.55/5 on 975k installs for
  vscode-bazel; 3.38/5 for the legacy JetBrains plugin.

### 5.2 Against — the counter-evidence, which is substantial

**Nobody who *has* starpls complains much.** 187 issues, max 6 reactions, 57 open, and
the maintainer can leave PRs from Bazel core contributors unmerged for a year without
anyone escalating. Compare: 21 reactions on a single *comment* on bazel PR #21929 asking
Google to unblock upstream. The energy is in "someone should do this", not in "the thing
that exists is inadequate for my daily work."

**The ICSE'24 abandonment study finds no editor theme at all.** Alfadel & McIntosh, *"The
Classics Never Go Out of Style: A Study on the Abandonment of Bazel"*, ICSE 2024,
<https://rebels.cs.uwaterloo.ca/papers/icse2024_alfadel.pdf>. 542 projects that adopted
Bazel; **61 (11.2%) abandoned it**, after a median of **638 days**. Seven emergent themes
from thematic analysis of 212 commits/issues/PRs:

| Theme | Projects | % |
|---|---|---|
| T1 Difficulty in maintenance and troubleshooting | 10 | 20% |
| T2 Lack of platform support and interoperability | 13 | 26% |
| T3 Replacement by toolchain-native build technology | 11 | 22% |
| T4 Experimental adoption | 2 | 4% |
| T5 Obstacles for external contributors / onboarding | 7 | 14% |
| T6 Bazel consumer no longer required | 3 | 6% |
| T7 Influence of upstream trends | 13 | 26% |

**None of the seven is "bad editor tooling."** T5 is the closest ("the biggest
disadvantage of adopting Bazel was the learning curve", pipe-cd/pipecd#1634). Editor
support is not why projects leave Bazel.

**Large orgs route around BUILD-file editing entirely.**
- **Spotify** (2023-10, <https://engineering.atspotify.com/2023/10/switching-build-systems-seamlessly>):
  > "we wrote scripts that **generated over 2,000 BUILD.bazel files** … **Only 30 to 50
  > BUILD.bazel files needed to be manually written**, which meant that most engineers
  > initially weren't even aware that we were running two build systems side by side."
- **Uber** (<https://www.uber.com/us/en/blog/go-monorepo-bazel/>): "With Gazelle, we are
  able to generate Bazel rules for most Go packages in our Go monorepo **with minimal
  human input**."
- **Google**: see §6 — `Glaze` ran on file-save; "Go users could mostly ignore BUILD
  files altogether."

If your generator works, you don't need an LSP for BUILD files, because you don't write
them. Gazelle has 1,412 stars and shipped a 99% speedup in Nov 2025 (§6.3). That is
arguably the competing product.

**AI agents are eating the "help me edit this BUILD file" job.**
- **Mercari** (2025-12-03,
  <https://engineering.mercari.com/en/blog/entry/20251202-shops-monorepo-five-years-later-a-tale-of-bazel-and-cursor/>)
  is the sharpest example. They describe exactly our target pain —
  > "The repository had become a 'maze.' New joiners faced a steep learning curve just to
  > run tests locally, and **developers were afraid to touch build files** lest they break
  > a service they didn't own."

  and then solve it with a rewrite plus **Cursor/Claude Code**, not tooling:
  > "Before the cleanup, when we asked an AI agent to 'add a new endpoint,' it would
  > fail. It couldn't understand our custom hacks… **The AI would hallucinate standard
  > Bazel rules that didn't exist in our custom setup.** After the cleanup, the repo was
  > 'boring'—and AI loves boring."
  > A junior with no Bazel experience asked Cursor *"How can I enable the golang race
  > detector in my Bazel build?"* and shipped it.
- **Databricks** (2026-08-11): *"Most code at Databricks is now written by agents."* And
  their own framing of what LSPs are now for:
  > "**AI changes the relative value of Language Server Protocol (LSP) features.** As
  > engineers write less code by hand, low setup cost, fast feedback, and reliable
  > codebase orientation matter more than broad autocomplete and refactoring coverage."
- **Google** ships `build_cleaner` in its agent instructions: `google/tpu-raiden`
  `AGENTS.md` — "Never try to prefix the following commands with anything, you should run
  them as is: `build_cleaner`, `blaze`."

**Nobody has built a Bazel MCP server that anyone uses.** Largest is 11 stars
(`aaomidi/mcp-bazel`). vscode-bazel#577 (2026-03-10, cbandera) is an open "is there
interest?" question with three replies and no consensus. So the agent story isn't
*solved* either — it's just being done ad hoc with terminal access.

**The two smaller competitors are stalling, and nobody noticed.** bazel-lsp's last human
commit was 2025-07-13 and its last release 2025-02-12; starpls hasn't shipped in 12
months. Neither has attracted a "this project is dead, I'm forking" issue. Low urgency.

### 5.3 My read

The market is **real but structurally mis-shaped**: demand is expressed *against the
editor extension*, not against the language server, and the bottleneck is
**distribution + maintainer bandwidth + upstream Bazel plumbing**, not "nobody has
written a good analyser."

Concretely:
- The Starlark-semantics half of the problem is **already solved to a high standard** by
  starpls. Rebuilding it is not where the value is.
- The build-graph half (references, rename, workspace symbols, add-missing-dep,
  unresolved-label diagnostics, all targets in the repo) is **unsolved by every OSS
  tool**, and the one attempt at it (`bazel query` on demand) is the top source of
  performance complaints in three independent codebases.
- Getting to ~1M users requires being *in* vscode-bazel by default — a social/packaging
  problem the community has failed at for eight years, and which four separate people
  asked for again between 2026-01 and 2026-03.
- If a new server can't beat "run Gazelle on save" or "ask Claude", it has no wedge.
  The defensible wedge is the thing Google has and nobody else does: **a fast, always-warm
  index of the whole target graph** (their `depserver`), which makes both
  correctness-checking-while-you-type and agent tool-calls cheap.

---

## 6. (e) What large Bazel users say publicly — i.e. what "good" looks like

### 6.1 Google: BUILD files are not hand-edited, and that is deliberate

**"Searching for Build Debt: Experiences Managing Technical Debt at Google"**,
Morgenthaler, Gridnev, Sauciuc, Bhansali (Google), MTD @ ICSE 2012,
<https://static.googleusercontent.com/media/research.google.com/en/us/pubs/archive/37755.pdf>
(text extracted locally). This is the founding document of Google's BUILD-file automation
and it is remarkably on-point:

> "BUILD files are for the most part manually maintained, and **this lack of automation
> can be a particular pain point for engineers, requiring non-trivial developer effort**."

> "Technical debt accumulates unless engineers are diligent to keep the source code and
> the dependencies, or `deps`, of their build targets synchronized."

Their stated principles:

> "**Automation.** Use automated techniques to analyze and (where possible) fix issues…
> **Make it easy to do the right thing.** … **if we can analyze changes that developers
> are about to make as part of their normal workflow (during editing, browsing, or code
> review), we can prevent certain kinds of debt.** … **Make it hard to do the wrong
> thing.**"

That middle sentence is, verbatim, the thesis of a Bazel language server — written in
2012 by the team that owned the problem.

Their mechanisms, in the paper:
- **Strict deps via the compiler**: "We began with Java, leveraging the `javac` compiler
  to tell the build system the classpath element from which each referenced class is
  loaded. … We then generate a warning or error each time the source code references a
  class from an indirect, transitive dependency, **including the name of that
  dependency**." (This shipped publicly as `strict_java_deps`.)
- Then: "Once under-declared dependencies are disallowed, the unneeded dependencies can
  be **automatically and safely removed**. The build system can then generate an error if
  it finds an unneeded dependency, effectively preventing this form of technical debt from
  recurring."
- **Clipper**, a "dependency refactoring assistant": "Clipper attempts to **fill the gap
  between the visualization tools and IDEs** by giving engineers refactoring guidance…
  Clipper makes suggestions by ranking dependencies in terms of high cost and low removal
  effort."
- **Zombie targets**: nightly query over every build result; after 90 days of failure a
  target "can be officially declared 'dead'" and deleted from the BUILD file.

### 6.2 `build_cleaner` — Google's actual answer, still not open source

Not documented publicly by Google, but its existence and role are well attested:

- **HN, yegle, 2024-06-16** <https://news.ycombinator.com/item?id=40695386>:
  > "There are 3 tools that that makes maintaining BUILD files enjoyable: **buildifier,
  > buildozer and build_cleaner (internal only unfortunately)**."
  (Directly answering "What is build_cleaner?" from lopkeny12ko in the same thread.)
- **HN, bjackman, 2024-02-22** <https://news.ycombinator.com/item?id=39464712>: gazelle
  "doesn't seem to support C++ or Python **in the way build_cleaner does**".
- **chipsalliance/verible PR #1505** (Google-adjacent): "The only way currently is to look
  at all the includes and add the relevant libraries to the BUILD file. **There is a tool
  `build_cleaner` that we use within Google, but I think it is not released anywhere :(**"
- **A leaked internal contract in OSS**: `pybind11_bazel/build_defs.bzl` contains
  `# Mark common dependencies as required for build_cleaner` /
  `tags = tags + ["req_dep=%s" % dep for dep in PYBIND_DEPS]`. So the tag vocabulary
  `req_dep=` is `build_cleaner`'s public footprint.
- **It is now part of Google's agent loop**: `google/tpu-raiden` `AGENTS.md` —
  "Never try to prefix the following commands with anything, you should run them as is:
  `build_cleaner`, `blaze`."
- **Android's public namesake**, `build/kernel/kleaf/build_cleaner.py`
  (<https://android.googlesource.com/kernel/build/+/refs/heads/main/kleaf/build_cleaner.py>)
  — "Kleaf `build_cleaner`: Fixes dependencies in BUILD files": run `bazel cquery`,
  compute missing deps, emit `buildozer` edits. Explicitly limited: "the script only works
  if the target is specified directly in BUILD or BUILD.bazel files. **It does not work if
  the target is wrapped in a macro.**"
- **OSS reimplementation:** `hzeller/bant` (30★, active 2026-08-22) advertises "Helps
  cleaning up BUILD dependencies by adding missing, and removing superfluous, dependencies
  ('`build_cleaner`'). Emits a `buildozer` script." Its author: *"I usually just don't even
  add `deps = [...]` manually anymore but just let `bant dwyu` do the work."*

### 6.3 `Glaze` — the 50 ms bar

Jay Conrod (EngFlow; formerly Google Go Tools, Gazelle maintainer), BazelCon 2025 talk,
blog edition **2025-11-25**
<https://blog.engflow.com/2025/11/25/lightning-fast-build-file-generation-with-gazelle-lazy-indexing/>:

> "Inside Google, there was a tool called **Glaze**, which generated `BUILD` files for Go
> packages. … It was very fast, **like 50ms, so people usually configured their editors to
> run it whenever they saved a file. So Go users could mostly ignore `BUILD` files
> altogether.** So Glaze is to Blaze as Gazelle is to Bazel."

And the design lesson, stated explicitly:

> "**Speed is an important feature for interactive tools.** … If it takes 50ms, you can run
> Gazelle automatically when you save a file. If it's 10s, you only run it when you have
> to. If it's 100s, you might validate your `BUILD` files in CI, but **you'll make most of
> your edits manually.** And if it's 1000s, don't even bother."

Measured Gazelle full-index times before lazy indexing: Kubernetes **30 s** in 2018 ("one
of the reasons they were unhappy with Bazel"), **Uber's monorepo 25 s**, EngFlow 839 ms,
Cockroach Labs 148 ms. After lazy indexing: EngFlow 107 ms, **Uber 352 ms (99%
improvement)** — "these numbers are running the Gazelle binary directly. When you invoke
it with `bazel run`, that adds 500–1000ms."

**This is the single most useful latency budget in the whole corpus.** An LSP that wants
to be on the edit path has to live in the 50–350 ms band, and `bazel run` alone blows
half of it.

### 6.4 `depserver` — Google's global build-graph service

**aiuto**, a Googler 2007–2025 who "worked on API serving infrastructure, desktop apps,
blaze/bazel", HN **2026-05-15** <https://news.ycombinator.com/item?id=48150872>:

> "Caveat: Code search and a monorepo let you do some amazing things. But there is a LOT
> of cost which Googlers tend to nostalgically gloss over. piper, citc, kythe, critique,
> and **depserver** represent (wet finger in the wind guess) probably **$100M of
> development effort**.
> \* **depserver is essentially a service that holds the entire blaze dependency graph
> every file up to every buildable object across the code space. That drives the automatic
> testing infrastructure.**"

Same thread, **dmoy** (Googler), 2026-05-16 <https://news.ycombinator.com/item?id=48160287>:
Kythe migration, "we did try to help them in the right direction, like **offloading onto
direct blaze depserver queries**."

aiuto also states the IDE-latency requirement directly:

> "Blaze had the information available through a query, but needed ways for **the user
> (the IDE) to optimize the query plan** so that it could deliver just the proto related
> dependencies with **sub-second response time to keep the human users happy**."

### 6.5 Cider — and the LSP Google never shipped

**Laurent Le Brun**, Google 2011–2024, Bazel/Starlark TL then Cider-V tech lead,
*"A History of IDEs at Google"*, **2026-05-09**
<https://laurent.le-brun.eu/blog/a-history-of-ides-at-google>
(HN 473 pts, <https://news.ycombinator.com/item?id=48073979>):

> "Although engineers used different IDEs, useful integrations eventually had to be
> reimplemented everywhere: **Bazel support, Starlark tooling, code formatters, code search
> integration**, and so on."
> "Traditional IDEs assumed that source code, build metadata, indexing and analysis all
> happened locally. **At Google scale, that assumption starts to break down.**"
> "The turning point came when they added support for code completion, **through the
> language-server protocol**. Cider was a light client … **All the magic happened on a
> backend that indexes the entire codebase**, so that all the data was ready whenever
> someone opened the webpage."
> "Code intelligence requires connecting each identifier with its type and references.
> This forms a huge language graph that has to be updated at every commit. … **the IDE
> also needs access to historical data** … my editor needs to use the graph corresponding
> to my last sync date… **augmented with my local changes**, obviously."
> "**by 2023, 80% of the development in the main Google codebase happened in Cider V**"
> "After two years, around **100 internal extensions** were being developed."

That Starlark LSP is the one laurentlb promised in vscode-bazel#1 in 2018 and retracted in
2020; alexeagle reported in 2022 that Googlers said it "may have been broken by the change
to the Monaco editor" and that the owning team "haven't even considered" open-sourcing it.

**Distilled "what good looks like" (all four are Google-internal):**
1. `Glaze`/`build_cleaner` write the `deps` for you, from source imports, in ~50 ms, on
   save. Humans barely touch BUILD files.
2. `strict_deps` in the compiler makes a wrong BUILD file a build error, with the correct
   label in the message.
3. `depserver` holds the *whole* dependency graph as a warm service, so
   references/rename/test-selection are sub-second queries, not `bazel query` spawns.
4. The editor is a thin client over that backend, with historical + local-overlay views.

### 6.6 Other large users

- **Databricks**, **2026-08-11**,
  <https://www.databricks.com/blog/open-sourcing-metals-v2-databricks-java-and-scala-language-server-multi-million-line-codebases>
  — "How closing the code intelligence gap made **Cursor** work for our **26M-line Bazel
  monorepo**". 285k Bazel JVM targets; internal (unreleased) Bazel BSP server; 92% of
  weekly-active IDE users on Cursor by 2026-07 vs 12% IntelliJ; "we did not renew the
  majority of our IntelliJ seats this year." Two directly relevant statements:
  > "Metals v2 **never invokes Bazel via the BSP server unless the user explicitly asks it
  > to.** In a large monorepo, background IDE sync can take the Bazel lock and compete
  > with developer-initiated" builds.
  > "AI changes the relative value of LSP features. As engineers write less code by hand,
  > **low setup cost, fast feedback, and reliable codebase orientation matter more than
  > broad autocomplete and refactoring coverage**."

  Note: this enormous investment went into **source-code** intelligence, not BUILD-file
  intelligence. Their BUILD-file contribution to the OSS world is a bug report:
  starpls#407.
- **Spotify**, 2023-10: 2,000+ generated BUILD files, 30–50 hand-written, 200+ iOS
  engineers, 120+ teams. Generated-not-edited.
- **Uber**: Gazelle for Go+proto, "minimal human input"; 70,000+ Go files; Gazelle
  indexing 25 s → 352 ms (2025).
- **Lyft** (Keith Smiley): uses and endorses starpls publicly (vscode-bazel#1,
  2025-04-17) and files PRs against it — one of the unmerged 20 (#428, 2026-06-03).
- **Canva**, <https://www.canva.dev/blog/engineering/faster-ci-builds-at-canva/>: "a build
  graph with over **900K nodes**"; "Loading and analyzing the build graph … from the
  `/WORKSPACE`, `**/BUILD.bazel`, and `*.bzl` files … **might take minutes**." — a hard
  ceiling on any LSP that wants to ask Bazel to load the graph.
- **Mercari**, 2025-12-03: see §5.2.
- **Stripe**, <https://stripe.dev/blog/fast-secure-builds-choose-two>: Bazel-based CI;
  internal rulesets for Ruby/JS/Terraform. No public statement on BUILD-file editing.
- **Dropbox / Figma / Snap**: I found **no** public statement about internal BUILD-file
  editing tooling. Dropbox's mobile build-system posts predate/omit Bazel-file authoring.

---

## 7. Status of every project in this space (as of 2026-08-25)

| Project | Last commit | Last release | Verdict |
|---|---|---|---|
| `withered-magic/starpls` | 2025-12-03 | v0.1.22, 2025-08-30 | **Stalling.** Best analyser; 20 open PRs incl. from Bazel core; formatting PR 12 months unmerged; no Bazel 9 builtins |
| `cameron-martin/bazel-lsp` | 2025-07-13 (human) | v0.6.4, 2025-02-12 | **Effectively frozen.** Renovate-bot only since Jul 2025 |
| `tilt-dev/starlark-lsp` | 2025-08-28 | — | Alive but tiny (35★, 9 issues ever); not Bazel-aware |
| `bazel-contrib/vscode-bazel` | 2026-08-24 | v0.14.0, 2026-03-31 | **Active**, new maintainer (cbandera); ~975k installs; still no bundled LSP |
| `stackb/bazel-stack-vscode` / `bzl` | 2023-08-07 | v1.9.8 | **Dead.** mason deprecated it 2025-12-01: "Servers offline" |
| `JetBrains/hirschgarten` | 2026-08-24 | GA 2025-07 | **Very active**, commercially staffed; issues on YouTrack (`BAZEL-*`), not GitHub |
| `bazelbuild/intellij` (legacy) | — | deprecated 2025-07 | JetBrains took over 2025-07; "no new features will be added" |
| `salesforce-misc/bazelrc-lsp` | 2026-06-02 | v0.2.6 | Active, narrow scope (`.bazelrc` only) |
| `bazelbuild/buildtools` | 2026-08-24 | v8.5.1, 2026-01-30 | **Very active.** 3.7M downloads of one release |
| `bazelbuild/vim-bazel` | 2022-04-09 | — | **Archived**, still linked from bazel.build/install/ide |
| `bazel-contrib/bazel.el` | 2026-08-20 | — | Active; syntax + buildifier, no LSP client |
| `zaucy/zed-starlark` | 2026-03-01 | v0.4.1 | Active; auto-installs starpls; 55k downloads |
| `hzeller/bant` | 2026-08-22 | on BCR | Active, tiny; the only OSS `build_cleaner` |
