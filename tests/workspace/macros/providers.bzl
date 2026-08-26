"""Providers, aspects, and the type-inference surface."""

MyInfo = provider(
    doc = "Carries a message and its length.",
    fields = {
        "message": "The string that was written.",
        "count": "len(message)",
    },
)

# Legacy schemaless provider — fields unknown statically.
LooseInfo = provider()

def _aspect_impl(target, ctx):
    transitive = []
    for dep in ctx.rule.attr.deps if hasattr(ctx.rule.attr, "deps") else []:
        if MyInfo in dep:
            transitive.append(dep[MyInfo].count)
    # Starlark has no `sum()`; the Python-shaped version fails at load time.
    total = 0
    for n in transitive:
        total += n
    return [LooseInfo(total = total)]

my_aspect = aspect(
    implementation = _aspect_impl,
    attr_aspects = ["deps"],
    provides = [LooseInfo],
)

def typed(x):
    """Return value type depends on the branch — a real inference challenge."""
    if x:
        return struct(kind = "a", value = 1)
    return struct(kind = "b", value = "one")

def uses_depset():
    d = depset([1, 2], transitive = [depset([3])])
    return d.to_list()
