"""A module extension. The repos it creates are invisible to static analysis."""

def _repo_impl(rctx):
    rctx.file("BUILD.bazel", 'filegroup(name = "ext_target", srcs = [])\n')
    rctx.file("WORKSPACE", "")

_generated_repo = repository_rule(implementation = _repo_impl)

def _my_ext_impl(mctx):
    for mod in mctx.modules:
        for _ in mod.tags.declare:
            pass
    _generated_repo(name = "ext_generated")
    _generated_repo(name = "ext_generated_two")

my_ext = module_extension(
    implementation = _my_ext_impl,
    tag_classes = {
        "declare": tag_class(attrs = {"name": attr.string()}),
    },
)
