mod support;

use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

use serde_json::{Value, json};
use support::Client;

#[test]
#[ignore = "requires BLS_WORKSPACE"]
fn probe_workspace() {
    let root = PathBuf::from(std::env::var_os("BLS_WORKSPACE").expect("BLS_WORKSPACE"));
    let relative = std::env::var("BLS_PROBE_FILE")
        .unwrap_or_else(|_| "environment/logging/BUILD.bazel".to_string());
    let text = std::fs::read_to_string(root.join(&relative)).expect("probe BUILD file");
    let static_label = "//cuttlefish/exported/cuttlefish/nodes:ASinModifierBounds_generation";
    let mut client = Client::spawn(&root);
    let initialized = client.initialize(&bazel_options());

    let responsive = Instant::now();
    client.open(&relative, 1, &text);
    client.wait_notification("textDocument/publishDiagnostics");
    let responsive_ms = responsive.elapsed().as_secs_f64() * 1_000.0;

    let ready = client.wait_workspace_symbol(static_label, static_label);
    let symbols = ready.value["result"].as_array().unwrap().len();
    let ready_ms = ready.elapsed.as_secs_f64() * 1_000.0;

    let mut workspace_latencies = Vec::new();
    for _ in 0..30 {
        workspace_latencies.push(
            client
                .request("workspace/symbol", &json!({"query": "logging"}))
                .elapsed
                .as_secs_f64()
                * 1_000.0,
        );
    }
    workspace_latencies.sort_by(f64::total_cmp);

    let uri = client.uri(&relative);
    let mut latencies = Vec::new();
    let mut document_symbols = 0;
    for _ in 0..30 {
        let response = client.request(
            "textDocument/documentSymbol",
            &json!({"textDocument": {"uri": uri}}),
        );
        document_symbols = response.value["result"].as_array().unwrap().len();
        latencies.push(response.elapsed.as_secs_f64() * 1_000.0);
    }
    latencies.sort_by(f64::total_cmp);

    let large_relative = std::env::var("BLS_LARGE_FILE")
        .unwrap_or_else(|_| "cuttlefish/exported/cuttlefish/nodes/BUILD.bazel".to_owned());
    let large_text = std::fs::read_to_string(root.join(&large_relative)).expect("large BUILD file");
    let large_uri = client.uri(&large_relative);
    let opened = Instant::now();
    client.open(&large_relative, 1, &large_text);
    client.wait_notification_matching("textDocument/publishDiagnostics", |note| {
        note["params"]["uri"] == large_uri && note["params"]["version"] == 1
    });
    let large_open_ms = opened.elapsed().as_secs_f64() * 1_000.0;
    let large_symbols = client.request(
        "textDocument/documentSymbol",
        &json!({"textDocument": {"uri": large_uri}}),
    );
    let cursor = large_text
        .rfind("//")
        .expect("a label in the large BUILD file")
        + 2;
    let (line, character) = position_at(&large_text, cursor);
    let large_hover = client.request(
        "textDocument/hover",
        &json!({
            "textDocument": {"uri": large_uri},
            "position": {"line": line, "character": character}
        }),
    );
    let rss_bytes = rss(client.pid());
    let stderr = client.shutdown();

    println!(
        "{}",
        json!({
            "workspace": root,
            "file": relative,
            "initialize_ms": initialized.elapsed.as_secs_f64() * 1_000.0,
            "responsive_ms": responsive_ms,
            "static_ready_ms": ready_ms,
            "document_symbol_ms": {
                "p50": percentile(&latencies, 50),
                "p95": percentile(&latencies, 95),
                "max": latencies.last()
            },
            "workspace_symbol_ms": {
                "p50": percentile(&workspace_latencies, 50),
                "p95": percentile(&workspace_latencies, 95),
                "max": workspace_latencies.last()
            },
            "document_symbols": document_symbols,
            "workspace_symbols": symbols,
            "large_file": large_relative,
            "large_file_bytes": large_text.len(),
            "large_open_ms": large_open_ms,
            "large_document_symbol_ms": large_symbols.elapsed.as_secs_f64() * 1_000.0,
            "large_document_symbols": large_symbols.value["result"].as_array().map(Vec::len),
            "large_hover_ms": large_hover.elapsed.as_secs_f64() * 1_000.0,
            "rss_bytes": rss_bytes,
            "server_errors": stderr.lines().filter(|line| line.contains("ERROR")).count()
        })
    );
}

fn bazel_options() -> Value {
    std::env::var("BLS_BAZEL_PATH").map_or_else(
        |_| json!({"bazel": {"enable": false}}),
        |path| json!({"bazel": {"enable": true, "path": path}}),
    )
}

fn percentile(values: &[f64], percentile: usize) -> f64 {
    values[(values.len() - 1) * percentile / 100]
}

fn position_at(text: &str, offset: usize) -> (u32, u32) {
    let prefix = &text[..offset];
    let line = u32::try_from(prefix.bytes().filter(|byte| *byte == b'\n').count()).unwrap();
    let current = prefix.rsplit_once('\n').map_or(prefix, |(_, line)| line);
    let character = u32::try_from(current.encode_utf16().count()).unwrap();
    (line, character)
}

fn rss(pid: u32) -> Option<u64> {
    let output = Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let kib = String::from_utf8(output.stdout)
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()?;
    Some(kib * 1_024)
}
