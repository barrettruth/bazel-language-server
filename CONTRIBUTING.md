# Contributing

## Scope

bazel-language-server is a language server for Bazel build files. It is not a
Starlark interpreter, a build tool, or a way to make other languages' language
servers work inside a Bazel repository.

Syntax belongs in [`starlark-cst`](https://github.com/barrettruth/starlark-cst),
which is a parser and nothing else. A change that needs to know what a string
refers to belongs here; a change to how a string is tokenised belongs there.

## Pull Requests

Bug fixes and documentation fixes are welcome. AI-generated contributions are
not accepted.

For new behavior, open an issue first unless the change is small and already
fits the project's scope.

Behavior or configuration changes should update `README.md` and the site under
`site/` when appropriate.

## Development

It is preferred to use the Nix development shell, which bundles all necessary
tools:

```sh
nix develop
```

## Checks

Run the local checks before opening a pull request:

```sh
nix develop --command just ci
```
