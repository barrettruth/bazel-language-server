//! Bazel 8.7 command names and option-inheritance relationships.

/// Bazel 8.7 commands plus the three rc-only scopes.
pub const NAMES: &[&str] = &[
    "always",
    "common",
    "startup",
    "analyze-profile",
    "aquery",
    "build",
    "canonicalize-flags",
    "clean",
    "config",
    "coverage",
    "cquery",
    "dump",
    "fetch",
    "help",
    "info",
    "license",
    "mobile-install",
    "mod",
    "print_action",
    "query",
    "run",
    "shutdown",
    "sync",
    "test",
    "vendor",
    "version",
];

/// Whether a declaration scoped to `defined` participates in `requested`.
#[must_use]
pub fn applies(requested: &str, defined: &str) -> bool {
    if matches!(defined, "always" | "common") || requested == defined {
        return true;
    }
    if matches!(requested, "always" | "common") {
        return true;
    }
    match requested {
        "coverage" => matches!(defined, "test" | "build"),
        "cquery" | "fetch" | "vendor" => matches!(defined, "test" | "build"),
        "aquery" | "canonicalize-flags" | "clean" | "config" | "info" | "mobile-install"
        | "print_action" | "run" | "test" => defined == "build",
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inheritance_is_transitive() {
        assert!(applies("coverage", "test"));
        assert!(applies("coverage", "build"));
        assert!(!applies("build", "test"));
        assert!(applies("query", "common"));
    }
}
