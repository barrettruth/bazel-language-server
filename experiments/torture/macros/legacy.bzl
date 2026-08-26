"""Legacy macros: plain Python-ish functions. Target names are computed at
evaluation time, so the set of targets a BUILD file declares is not knowable
by reading the BUILD file alone."""

def legacy_macro(name, count = 3, **kwargs):
    # N targets from one call site. An LSP that does not evaluate cannot know
    # `//lib:from_legacy_0` exists.
    for i in range(count):
        native.filegroup(
            name = "%s_%d" % (name, i),
            srcs = [],
            **kwargs
        )

    native.filegroup(
        name = name,
        srcs = ["%s_%d" % (name, i) for i in range(count)],
        **kwargs
    )

def name_from_dict(prefix, entries):
    # Target names built from a dict comprehension — worse still.
    for key, value in entries.items():
        native.genrule(
            name = prefix + "_" + key,
            outs = [prefix + "_" + key + ".txt"],
            cmd = "echo %s > $@" % value,
        )
