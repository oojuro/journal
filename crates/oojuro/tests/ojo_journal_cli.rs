#![cfg(target_os = "linux")]
//! Actual .ojo application subprocesses; synthetic files and no provider access.
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::atomic::{AtomicU64, Ordering},
};
const BIN: &str = env!("CARGO_BIN_EXE_ojo-journal");
const LEGACY: &str = env!("CARGO_BIN_EXE_oojuro-native");
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "ojo-cli-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }
    fn root(&self) -> PathBuf {
        self.0.join("root")
    }
    fn call(&self, verb: &str, args: &[&str]) -> Output {
        Command::new(BIN)
            .arg(verb)
            .args(["--root", self.root().to_str().unwrap()])
            .args(args)
            .output()
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn ok(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn original_context_export_import_archive_and_mcp_work_through_the_real_cli() {
    let f = Fixture::new();
    ok(f.call("init", &[]));
    let text = "Exact café\0 decision\r\nTool result: status=0\n";
    let input = f.0.join("original.txt");
    fs::write(&input, text).unwrap();
    let arguments = [
        "--session-id",
        "session-one",
        "--event-id",
        "event-one",
        "--text-file",
        input.to_str().unwrap(),
    ];
    let appended = ok(f.call("record", &arguments));
    assert_eq!(appended["stored"], 1);
    let run = appended["session"].as_str().unwrap();
    assert_eq!(ok(f.call("record", &arguments))["stored"], 0);
    assert_eq!(
        ok(f.call("get", &["--session", run, "--id", "0"]))["input"]["text"],
        text
    );
    assert_eq!(
        ok(f.call("search", &["--query", "café decision"]))["matches"][0]["input"]["text"],
        text
    );
    assert_eq!(
        ok(f.call("context", &["--session", run, "--id", "0"]))["records"][0]["input"]["text"],
        text
    );
    let exported = f.call("export", &["--session", run]);
    assert!(exported.status.success());
    let export = f.0.join("export.jsonl");
    fs::write(&export, &exported.stdout).unwrap();
    let other = Fixture::new();
    ok(other.call("init", &[]));
    assert_eq!(
        ok(other.call("import", &["--file", export.to_str().unwrap()]))["imported"],
        1
    );
    assert_eq!(
        ok(other.call("import", &["--file", export.to_str().unwrap()]))["deduplicated"],
        1
    );
    assert_eq!(
        ok(other.call("get", &["--session", run, "--id", "0"]))["input"]["text"],
        text
    );
    ok(f.call("finish", &["--session", run]));
    ok(f.call("archive", &["--session", run]));
    assert_eq!(
        ok(f.call("get", &["--session", run, "--id", "0"]))["input"]["text"],
        text
    );
    let mut child = Command::new(BIN)
        .args(["mcp", "--root", f.root().to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let requests = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"get","arguments":{}}}),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"get","arguments":{"session":run,"id":0}}}),
    ];
    {
        let mut stdin = child.stdin.take().unwrap();
        for request in requests {
            writeln!(stdin, "{request}").unwrap();
        }
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let responses = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(responses.len(), 3);
    assert!(responses[1]["error"].is_object());
    let original: Value = serde_json::from_str(
        responses[2]["result"]["content"][0]["text"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(original["input"]["text"], text);
    let file = f.root().join("journals").join(format!("{run}.ojo"));
    assert_eq!(&fs::read(file).unwrap()[..8], b"OJOFILE\0");
}
fn write(path: &Path, value: Value) {
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    writeln!(file, "{value}").unwrap();
}
#[test]
fn one_command_connect_and_actual_capture_use_one_ojo_per_provider_session() {
    let f = Fixture::new();
    let sources = f.0.join("sources");
    fs::create_dir(&sources).unwrap();
    let connection = ok(f.call(
        "connect",
        &[
            "--adapter",
            "codex",
            "--source-root",
            sources.to_str().unwrap(),
            "--accept-local-capture",
            "--no-service",
        ],
    ));
    assert_eq!(
        connection["verification"]["parser_and_local_durability_verified"],
        true
    );
    assert_eq!(connection["verification"]["synthetic"], true);
    let first = sources.join("rollout-one.jsonl");
    write(
        &first,
        json!({"type":"session_meta","payload":{"id":"synthetic-provider-one"}}),
    );
    write(
        &first,
        json!({"type":"response_item","timestamp":"2026-10-24T12:00:00Z","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"recorded decision"}]}}),
    );
    let report = ok(f.call("capture", &["--adapter", "codex"]));
    assert_eq!(report["stored"], 1);
    let run = report["sessions"][0].as_str().unwrap();
    assert_eq!(
        ok(f.call("get", &["--session", run, "--id", "0"]))["input"]["text"],
        "recorded decision"
    );
    assert_eq!(ok(f.call("capture", &["--adapter", "codex"]))["stored"], 0);
    assert!(!f.root().join("cloud.json").exists());
    assert_eq!(fs::read_dir(f.root().join("journals")).unwrap().count(), 2);
}

fn inventory(root: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, out: &mut std::collections::BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                out.insert(
                    path.strip_prefix(root).unwrap().into(),
                    fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut out = std::collections::BTreeMap::new();
    walk(root, root, &mut out);
    out
}
#[test]
fn legacy_migration_preserves_the_source_and_restores_every_original_file_byte() {
    let f = Fixture::new();
    ok(f.call("init", &[]));
    let source = f.0.join("legacy-trace");
    {
        let mut journal = oojuro::Journal::open(&source).unwrap();
        journal
            .remember(
                "synthetic:writer",
                "legacy exact café\r\noriginal",
                None,
                Some(1),
            )
            .unwrap();
        journal
            .remember("synthetic:writer", "second original", None, Some(2))
            .unwrap();
    }
    let before = inventory(&source);
    let migrated = ok(f.call(
        "migrate",
        &[
            "--legacy-root",
            source.to_str().unwrap(),
            "--allow-raw-source",
        ],
    ));
    assert_eq!(migrated["records"], 2);
    assert_eq!(inventory(&source), before);
    let repeated = ok(f.call(
        "migrate",
        &[
            "--legacy-root",
            source.to_str().unwrap(),
            "--allow-raw-source",
        ],
    ));
    assert_eq!(repeated["records"], 0);
    assert_eq!(repeated["deduplicated"], 2);
    assert_eq!(
        ok(f.call("search", &["--query", "legacy exact"]))["matches"][0]["input"]["text"],
        "legacy exact café\r\noriginal"
    );
    let restored = f.0.join("restored");
    ok(f.call(
        "restore-migration",
        &[
            "--migration",
            migrated["recovery_artifact"].as_str().unwrap(),
            "--out",
            restored.to_str().unwrap(),
        ],
    ));
    assert_eq!(inventory(&restored), before);
    {
        let journal = oojuro::Journal::open_existing(&restored).unwrap();
        assert_eq!(
            journal.reader().get_id(1).unwrap().unwrap().text,
            "legacy exact café\r\noriginal"
        );
    }
    assert!(
        !f.call(
            "restore-migration",
            &[
                "--migration",
                migrated["recovery_artifact"].as_str().unwrap(),
                "--out",
                restored.to_str().unwrap()
            ]
        )
        .status
        .success()
    );
    assert!(
        !f.call("migrate", &["--legacy-root", source.to_str().unwrap()])
            .status
            .success()
    );
    assert_eq!(inventory(&source), before);
    assert!(!fs::read_dir(f.root()).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".migration-work-")
    }));
}
fn legacy(root: &Path, verb: &str, args: &[&str]) -> Value {
    ok(Command::new(LEGACY)
        .arg(verb)
        .args(["--root", root.to_str().unwrap()])
        .args(args)
        .output()
        .unwrap())
}
fn private_write(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}
#[test]
fn native_capture_and_record_namespaces_migrate_with_frozen_source_backups() {
    for layout in ["native", "record"] {
        let f = Fixture::new();
        ok(f.call("init", &[]));
        let source = f.0.join("legacy");
        if layout == "native" {
            let policy = f.0.join("policy.json");
            private_write(&policy,serde_json::to_vec(&json!({"version":1,"record":true,"paused":false,"record_thinking":true,"index_thinking":true,"exclude":[],"denied_tools":[]})).unwrap().as_slice());
            legacy(
                &source,
                "setup",
                &[
                    "--harness",
                    "codex",
                    "--writer",
                    "codex:-synthetic",
                    "--agent",
                    "7",
                    "--policy",
                    policy.to_str().unwrap(),
                ],
            );
            let transcript = f.0.join("rollout-native.jsonl");
            write(
                &transcript,
                json!({"type":"session_meta","payload":{"id":"native-session","cwd":"/synthetic"}}),
            );
            write(
                &transcript,
                json!({"type":"response_item","timestamp":"2026-10-24T12:00:00Z","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"native exact original café\r\n"}]}}),
            );
            legacy(
                &source,
                "capture",
                &[
                    "--source",
                    transcript.to_str().unwrap(),
                    "--source-id",
                    "11111111111111111111111111111111",
                    "--allow-raw-source",
                ],
            );
        } else {
            legacy(
                &source,
                "record-setup",
                &[
                    "--writer",
                    "synthetic-writer",
                    "--owner",
                    "7",
                    "--adapter",
                    "synthetic/v1",
                ],
            );
            let input = f.0.join("record.json");
            private_write(&input,serde_json::to_vec(&json!([{"agent_id":"synthetic-agent","parent_id":null,"valid_at":1,"kind":"explicit","tool":null,"status":null,"exit":null,"key":null,"text":"record exact original café\r\n","attrs":{},"indexed":true}])).unwrap().as_slice());
            legacy(
                &source,
                "record-append",
                &[
                    "--input",
                    input.to_str().unwrap(),
                    "--request",
                    "synthetic",
                    "--allow-unredacted-records",
                ],
            );
        }
        let before = inventory(&source);
        let migrated = ok(f.call(
            "migrate",
            &[
                "--legacy-root",
                source.to_str().unwrap(),
                "--allow-raw-source",
            ],
        ));
        assert_eq!(migrated["records"], 1, "{layout}");
        assert_eq!(inventory(&source), before);
        let hits = ok(f.call("search", &["--query", "exact original"]));
        assert_eq!(hits["matches"].as_array().unwrap().len(), 1);
        assert!(
            hits["matches"][0]["input"]["text"]
                .as_str()
                .unwrap()
                .contains("café\r\n")
        );
        let restored = f.0.join("restored");
        ok(f.call(
            "restore-migration",
            &[
                "--migration",
                migrated["recovery_artifact"].as_str().unwrap(),
                "--out",
                restored.to_str().unwrap(),
            ],
        ));
        assert_eq!(inventory(&restored), before);
        let repeated = ok(f.call(
            "migrate",
            &[
                "--legacy-root",
                source.to_str().unwrap(),
                "--allow-raw-source",
            ],
        ));
        assert_eq!(repeated["deduplicated"], 1);
        assert_eq!(repeated["records"], 0);
    }
}
