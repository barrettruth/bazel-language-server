# Builtin-knowledge artifacts

Produced 2026-08-25 on macOS arm64 with `bazel 9.2.0` (downloaded from the GitHub
release) and `protoc` 35.1 (`nix shell nixpkgs#protobuf`).

## Artifacts

| File | Bytes | How it was produced |
|---|---|---|
| `bazel-9.2.0-builtin.pb` | 613 067 | `git clone --depth 1 -b 9.2.0` + `bazel build //src/main/java/com/google/devtools/build/lib:gen_api_proto` (143 s) |
| `bazel-10.0.0-prerelease-builtin.pb` | 619 936 | same, from `upstream/bazel` @ `3a9b19c8` (2026-08-24), 216 s |
| `starpls-builtin.pb` | 2 452 170 | copied from `upstream/starpls/crates/starpls/src/builtin/builtin.pb` (checked in 2024-12-27, ≈ Bazel 8.0) |
| `bazel-9.2.0-build-language.pb` | 33 569 | `bazel info build-language` in an empty bzlmod workspace |
| `bazel-9.2.0-query-rule_classes.pb` | 28 117 | `bazel query --output=proto --proto:rule_classes=true //:all` in a workspace with `rules_cc 0.2.19` |
| `bazel-skylib-query-rule_classes.pb` | 947 324 | same over `//...` of bazel-skylib @ HEAD (437 targets, 140 rule classes, 0.14 s warm) |
| `user-module-doc_extract.binaryproto` | 431 | `starlark_doc_extract` on a hand-written `doc.bzl` |
| `rules_cc-cc_library-doc_extract.binaryproto` | 117 | `starlark_doc_extract` on `@rules_cc//cc:cc_library.bzl` — **useless**, that file only holds a `**kwargs` wrapper macro |
| `rules_cc-cc_library-impl-doc_extract.binaryproto` | 25 642 | `starlark_doc_extract` on `@rules_cc//cc/private/rules_impl:cc_library.bzl` — the real rule, 26 attributes |
| `rules_js-v3.4.1.docs.tar.gz` + `bcr-docs-sample/` | 29 287 | BCR `docs_url` artifact for `aspect_rules_js` 3.4.1; contains 5 `*.doc_extract.binaryproto` (`stardoc_output.ModuleInfo`) |
| `protos/` | | `builtin.proto`, `build.proto`, `stardoc_output.proto` copied from Bazel HEAD; `protos/src/main/protobuf/` mirrors the import path `build.proto` needs |

`*.txtpb` files are `protoc --decode` renderings of the corresponding `.pb`.

## Decoding

```sh
nix shell nixpkgs#protobuf -c protoc --decode=builtin.Builtins \
  --proto_path=protos protos/builtin.proto < bazel-9.2.0-builtin.pb

nix shell nixpkgs#protobuf -c protoc --decode=blaze_query.BuildLanguage \
  --proto_path=protos protos/src/main/protobuf/build.proto < bazel-9.2.0-build-language.pb

nix shell nixpkgs#protobuf -c protoc --decode=blaze_query.QueryResult \
  --proto_path=protos protos/src/main/protobuf/build.proto < bazel-9.2.0-query-rule_classes.pb

nix shell nixpkgs#protobuf -c protoc --decode=stardoc_output.ModuleInfo \
  --proto_path=protos protos/stardoc_output.proto < user-module-doc_extract.binaryproto
```

`build.proto` imports `src/main/protobuf/stardoc_output.proto`, hence the nested
`protos/src/main/protobuf/` copy.
