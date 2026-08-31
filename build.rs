//! Generates Rust for the vendored Bazel protocols.
//!
//! `protox` rather than `protoc`, which `prost-build` otherwise shells out to.
//! There is no `protoc` in this flake and none on the machine of anyone who
//! ran `cargo install bazel-language-server`, so a build that needs one is a
//! build that fails for the people who install the ordinary way.
//!
//! `build.proto` comes from the oldest Bazel this server drives and decodes
//! newer output because prost skips fields it does not know. `bazel_flags.proto`
//! is the exact Bazel 8.7 schema whose flag catalog the server advertises.

/// Relative to `proto/`, which is the include root because `build.proto`
/// imports its sibling by the path Bazel's own source tree gives it.
const PROTOS: [&str; 3] = [
    "src/main/protobuf/build.proto",
    "src/main/protobuf/bazel_flags.proto",
    "src/main/protobuf/stardoc_output.proto",
];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    for proto in PROTOS {
        println!("cargo::rerun-if-changed=proto/{proto}");
    }
    prost_build::Config::new().compile_fds(protox::compile(PROTOS, ["proto"])?)?;
    Ok(())
}
