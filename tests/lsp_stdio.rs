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
    let diagnostics = client
        .wait_notification_matching("textDocument/publishDiagnostics", |note| {
            note["params"]["version"] == 1
        });
    assert!(diagnostics["params"]["diagnostics"].is_array());

    let symbols = client.request(
        "textDocument/documentSymbol",
        &json!({"textDocument": {"uri": client.uri(relative)}}),
    );
    assert!(symbols.value["result"].as_array().unwrap().len() >= 2);

    let label = "//lib/sub:sub_srcs";
    client.wait_workspace_symbol(label, label);
    let indexed = client.wait_notification_matching("$/progress", |note| {
        note["params"]["value"]["kind"] == "end"
    });
    assert!(
        indexed["params"]["value"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("BUILD files"))
    );
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

    let (end_line, end_character) = position(&text, "");
    client.notify(
        "textDocument/didChange",
        &json!({
            "textDocument": {"uri": client.uri(relative), "version": 2},
            "contentChanges": [{
                "range": {
                    "start": {"line": end_line, "character": end_character},
                    "end": {"line": end_line, "character": end_character}
                },
                "text": "filegroup(name = "
            }]
        }),
    );
    let diagnostics = client
        .wait_notification_matching("textDocument/publishDiagnostics", |note| {
            note["params"]["version"] == 2
        });
    assert!(
        diagnostics["params"]["diagnostics"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );

    client.notify(
        "textDocument/didClose",
        &json!({"textDocument": {"uri": client.uri(relative)}}),
    );
    let cleared = client.wait_notification_matching("textDocument/publishDiagnostics", |note| {
        note["params"]["version"] == 2 && note["params"]["diagnostics"] == json!([])
    });
    assert_eq!(cleared["params"]["diagnostics"], json!([]));
    let stderr = client.shutdown();
    assert!(!stderr.contains("panicked"), "{stderr}");
}

#[test]
fn cancellation_is_answered_while_formatting_runs() {
    let root = fixture();
    let relative = "generated/BUILD.bazel";
    let text = "filegroup(name = \"x\", srcs = [])\n".repeat(40_000);
    let mut client = Client::spawn(&root);
    client.initialize(&json!({"bazel": {"enable": false}}));
    client.open(relative, 1, &text);
    client.wait_notification_matching("textDocument/publishDiagnostics", |note| {
        note["params"]["version"] == 1
    });

    let request = client.send_request(
        "textDocument/formatting",
        &json!({
            "textDocument": {"uri": client.uri(relative)},
            "options": {"tabSize": 4, "insertSpaces": true}
        }),
    );
    client.notify("$/cancelRequest", &json!({"id": request}));
    let response = client.wait_response(request);
    assert_eq!(response["error"]["code"], -32800, "{response}");

    client.notify(
        "textDocument/didClose",
        &json!({"textDocument": {"uri": client.uri(relative)}}),
    );
    client.shutdown();
}

#[test]
fn stdio_session_serves_bazelrc_structure() {
    let root = fixture();
    let relative = ".bazelrc";
    let text = std::fs::read_to_string(root.join(relative)).unwrap();
    let mut client = Client::spawn(&root);
    let initialized = client.initialize(&json!({"bazel": {"enable": false}}));
    assert_eq!(
        initialized
            .value
            .pointer("/result/capabilities/completionProvider/triggerCharacters"),
        Some(&json!(["-", "="]))
    );
    client.open(relative, 1, &text);
    client.wait_notification_matching("textDocument/publishDiagnostics", |note| {
        note["params"]["version"] == 1
    });
    client.wait_notification_matching("$/progress", |note| {
        note["params"]["value"]["kind"] == "end"
    });

    assert_bazelrc_navigation(&mut client, relative, &text);
    assert_bazelrc_structure(&mut client, relative, &text);

    client.notify(
        "textDocument/didClose",
        &json!({"textDocument": {"uri": client.uri(relative)}}),
    );
    client.shutdown();
}

fn assert_bazelrc_navigation(client: &mut Client, relative: &str, text: &str) {
    let (line, character) = position(text, "shared");
    let completion = client.request(
        "textDocument/completion",
        &json!({
            "textDocument": {"uri": client.uri(relative)},
            "position": {"line": line, "character": character}
        }),
    );
    let labels: Vec<_> = completion.value["result"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|item| item["label"].as_str())
        .collect();
    assert!(labels.contains(&"shared"), "{completion:?}");
    assert!(!labels.contains(&"query-only"), "{completion:?}");

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
            .is_some_and(|links| links.iter().any(|link| link["targetUri"]
                .as_str()
                .is_some_and(|uri| uri.ends_with("/config/defaults.bazelrc")))),
        "{definition:?}"
    );

    let (line, character) = position(text, "config/defaults.bazelrc");
    let links = client.request(
        "textDocument/documentLink",
        &json!({"textDocument": {"uri": client.uri(relative)}}),
    );
    assert!(
        links.value["result"]
            .as_array()
            .is_some_and(|links| links.len() == 1),
        "{links:?}"
    );
    let import_definition = client.request(
        "textDocument/definition",
        &json!({
            "textDocument": {"uri": client.uri(relative)},
            "position": {"line": line, "character": character}
        }),
    );
    assert!(import_definition.value["result"].is_array());
}

fn assert_bazelrc_structure(client: &mut Client, relative: &str, text: &str) {
    let (line, character) = position(text, "config/defaults.bazelrc");
    let tokens = client.request(
        "textDocument/semanticTokens/full",
        &json!({"textDocument": {"uri": client.uri(relative)}}),
    );
    assert!(
        tokens.value["result"]["data"]
            .as_array()
            .is_some_and(|tokens| !tokens.is_empty())
    );
    let folds = client.request(
        "textDocument/foldingRange",
        &json!({"textDocument": {"uri": client.uri(relative)}}),
    );
    assert!(
        folds.value["result"]
            .as_array()
            .is_some_and(|folds| !folds.is_empty())
    );
    let selection = client.request(
        "textDocument/selectionRange",
        &json!({
            "textDocument": {"uri": client.uri(relative)},
            "positions": [{"line": line, "character": character}]
        }),
    );
    assert_eq!(selection.value["result"].as_array().map(Vec::len), Some(1));

    let (line, character) = position(text, "");
    let commands = client.request(
        "textDocument/completion",
        &json!({
            "textDocument": {"uri": client.uri(relative)},
            "position": {"line": line, "character": character}
        }),
    );
    let labels: Vec<_> = commands.value["result"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|item| item["label"].as_str())
        .collect();
    assert!(labels.contains(&"startup"));
    assert!(labels.contains(&"try-import-if-bazel-version"));
}

fn position(text: &str, needle: &str) -> (u32, u32) {
    let offset = if needle.is_empty() {
        text.len()
    } else {
        text.find(needle).expect("fixture needle")
    };
    let prefix = &text[..offset];
    let line = u32::try_from(prefix.bytes().filter(|byte| *byte == b'\n').count()).unwrap();
    let current = prefix.rsplit_once('\n').map_or(prefix, |(_, line)| line);
    let character = u32::try_from(current.encode_utf16().count()).unwrap();
    (line, character + u32::from(!needle.is_empty()) * 2)
}
