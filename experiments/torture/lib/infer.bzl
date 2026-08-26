"""Escalating type-inference cases. Each PROBE_n marks a hover position."""

MyInfo = provider(fields = {"message": "a string", "count": "an int"})

# 1 — trivial literal
easy = 1

# 2 — struct field access
s = struct(name = "lib", version = 3)
easy_field = s.version

# 3 — provider field off a dependency
def _read_provider(dep):
    got = dep[MyInfo]
    return got.message

# 4 — depset round trip
def _depset_case():
    d = depset(["a"], transitive = [depset(["b"])])
    items = d.to_list()
    return items

# 5 — the branch problem: two different struct shapes
def branchy(flag):
    if flag:
        return struct(kind = "a", value = 1)
    return struct(kind = "b", value = "one", extra = True)

merged = branchy(True)
merged_value = merged.value

# 6 — ctx.attr: type comes from the rule's own attrs dict, defined below
def _impl(ctx):
    who = ctx.attr.who
    n = ctx.attr.times
    files = ctx.files.srcs
    out = ctx.actions.declare_file("x")
    return [DefaultInfo(files = depset([out]))]

greeter = rule(
    implementation = _impl,
    attrs = {
        "who": attr.string(default = "world"),
        "times": attr.int(default = 1),
        "srcs": attr.label_list(allow_files = True),
    },
)

# 7 — select(): value is configuration-dependent, unknown until analysis
maybe = select({"//conditions:default": ["a"]})

# 8 — **kwargs passthrough erases everything
def wrapper(name, **kwargs):
    greeter(name = name, **kwargs)

# 9 — a dict built by a loop, then indexed
def _dyn():
    table = {}
    for k in ["x", "y"]:
        table[k] = struct(label = Label("//lib:" + k))
    return table["x"].label
