//! Generates Rust for the query protocol from Bazel's own `.proto` files.
//!
//! `protox` rather than `protoc`, which `prost-build` otherwise shells out to.
//! There is no `protoc` in this flake and none on the machine of anyone who
//! ran `cargo install bazel-language-server`, so a build that needs one is a
//! build that fails for the people who install the ordinary way.
//!
//! The protos are vendored from the tag named by `bazel::FLOOR`, the oldest
//! Bazel this server drives, and decode newer output correctly: every change to
//! `build.proto` since 6.5 has added fields, and prost skips the ones it does
//! not know.

/// Relative to `proto/`, which is the include root because `build.proto`
/// imports its sibling by the path Bazel's own source tree gives it.
const PROTOS: [&str; 2] = [
    "src/main/protobuf/build.proto",
    "src/main/protobuf/stardoc_output.proto",
];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    for proto in PROTOS {
        println!("cargo::rerun-if-changed=proto/{proto}");
    }
    prost_build::Config::new().compile_fds(protox::compile(PROTOS, ["proto"])?)?;
    Ok(())
}
