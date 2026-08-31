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

/// Whether an rc entry can select a named configuration.
#[must_use]
pub fn accepts_config(command: &str) -> bool {
    command != "startup" && NAMES.contains(&command)
}

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

/// Rc scopes Bazel 8.7 consults for one concrete command, in expansion order.
#[must_use]
pub fn scopes(command: &str) -> Vec<&str> {
    if !NAMES.contains(&command) {
        return Vec::new();
    }
    if matches!(command, "always" | "common" | "startup") {
        return vec![command];
    }
    let mut scopes = vec!["always", "common"];
    for inherited in ["build", "test"] {
        if inherited != command && applies(command, inherited) {
            scopes.push(inherited);
        }
    }
    scopes.push(command);
    scopes
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

    #[test]
    fn startup_cannot_select_a_named_configuration() {
        assert!(!accepts_config("startup"));
        assert!(!accepts_config("future-command"));
        assert!(accepts_config("build"));
        assert!(accepts_config("common"));
    }

    #[test]
    fn scopes_are_ordered_from_broadest_to_invoked() {
        assert_eq!(scopes("build"), ["always", "common", "build"]);
        assert_eq!(
            scopes("coverage"),
            ["always", "common", "build", "test", "coverage"]
        );
    }
}
