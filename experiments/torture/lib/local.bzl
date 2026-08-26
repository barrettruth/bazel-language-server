"""Rules loaded by sibling BUILD via a relative `:local.bzl` label."""

load("//macros:providers.bzl", "MyInfo")

def _impl(ctx):
    out = ctx.actions.declare_file(ctx.label.name + ".txt")
    ctx.actions.write(output = out, content = ctx.attr.message)
    return [
        DefaultInfo(files = depset([out])),
        MyInfo(message = ctx.attr.message, count = len(ctx.attr.message)),
    ]

_local_rule = rule(
    implementation = _impl,
    attrs = {
        "message": attr.string(default = "hi", doc = "Text to write."),
        "dep": attr.label(
            # A default that is itself a label — goto-def should work here.
            default = "//lib:srcs",
            allow_files = True,
        ),
        "deps": attr.label_list(providers = [MyInfo]),
        "_implicit": attr.label(
            default = Label("@bazel_tools//tools/bash/runfiles"),
            executable = False,
        ),
    },
    provides = [MyInfo],
    doc = "Writes a message to a file.",
)

def local_helper(name, **kwargs):
    _local_rule(name = name, **kwargs)

def renamed_in_load(name, **kwargs):
    _local_rule(name = name, message = "renamed", **kwargs)
