"""Symbolic macros (Bazel 8+): the construct that exists *because* legacy macros
are opaque to tooling. A language server can introspect these; it cannot
introspect the legacy kind."""

load("//lib:local.bzl", "local_helper")

def _symbolic_impl(name, visibility, dep, extra_tags, **kwargs):
    local_helper(
        name = name,
        message = "symbolic",
        visibility = visibility,
    )
    native.filegroup(
        name = name + "_group",
        srcs = [dep],
        tags = extra_tags,
        visibility = visibility,
    )

symbolic_macro = macro(
    implementation = _symbolic_impl,
    attrs = {
        "dep": attr.label(mandatory = True, configurable = False),
        # Symbolic-macro attrs are configurable by default, which makes them
        # `select()`-wrapped and illegal to pass to a non-configurable native
        # attribute such as `tags`.
        "extra_tags": attr.string_list(default = [], configurable = False),
    },
    doc = "Declares `{name}` and `{name}_group`.",
)

def _finalizer_impl(name, visibility, **kwargs):
    native.filegroup(
        name = name,
        srcs = native.existing_rules().keys(),
        visibility = visibility,
    )

# Finalizers run last and may call native.existing_rules() — inherently
# un-analysable without evaluating the whole package.
collect_all = macro(
    implementation = _finalizer_impl,
    finalizer = True,
)
