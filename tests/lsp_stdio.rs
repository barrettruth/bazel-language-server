mod support;

use std::path::PathBuf;

use serde_json::json;
use support::Client;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/workspace")
}

#[test]
fn stdio_session_serves_navigation_and_diagnostics() {
    let root = fixture();
    let relative = "lib/BUILD.bazel";
    let text = std::fs::read_to_string(root.join(relative)).unwrap();
    let mut client = Client::spawn(&root);
    let initialized = client.initialize(&json!({"bazel": {"enable": false}}));
    assert_eq!(
        initialized.value.pointer("/result/serverInfo/name"),
        Some(&json!("bazel-language-server"))
    );

    client.open(relative, 1, &text);
    let diagnostics = client.wait_notification("textDocument/publishDiagnostics");
    assert!(diagnostics["params"]["diagnostics"].is_array());

    let symbols = client.request(
        "textDocument/documentSymbol",
        &json!({"textDocument": {"uri": client.uri(relative)}}),
    );
    assert!(symbols.value["result"].as_array().unwrap().len() >= 2);

    let label = "//lib/sub:sub_srcs";
    let (line, character) = position(&text, label);
    let definition = client.request(
        "textDocument/definition",
        &json!({
            "textDocument": {"uri": client.uri(relative)},
            "position": {"line": line, "character": character}
        }),
    );
    assert!(
        definition.value["result"]
            .as_array()
            .is_some_and(|links| !links.is_empty())
    );

    let references = client.request(
        "textDocument/references",
        &json!({
            "textDocument": {"uri": client.uri(relative)},
            "position": {"line": line, "character": character},
            "context": {"includeDeclaration": true}
        }),
    );
    assert!(
        references.value["result"]
            .as_array()
            .is_some_and(|refs| !refs.is_empty())
    );

    let broken = format!("{text}\nfilegroup(name = ");
    client.notify(
        "textDocument/didChange",
        &json!({
            "textDocument": {"uri": client.uri(relative), "version": 2},
            "contentChanges": [{"text": broken}]
        }),
    );
    let diagnostics = client.wait_notification("textDocument/publishDiagnostics");
    assert!(
        diagnostics["params"]["diagnostics"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );

    client.notify(
        "textDocument/didClose",
        &json!({"textDocument": {"uri": client.uri(relative)}}),
    );
    let stderr = client.shutdown();
    assert!(!stderr.contains("panicked"), "{stderr}");
}

fn position(text: &str, needle: &str) -> (u32, u32) {
    let offset = text.find(needle).expect("fixture needle");
    let prefix = &text[..offset];
    let line = u32::try_from(prefix.bytes().filter(|byte| *byte == b'\n').count()).unwrap();
    let current = prefix.rsplit_once('\n').map_or(prefix, |(_, line)| line);
    let character = u32::try_from(current.encode_utf16().count()).unwrap();
    (line, character + 2)
}
