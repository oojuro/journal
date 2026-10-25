#![cfg(target_os = "linux")]
//! Actual .juro application subprocesses; synthetic files and no provider access.
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
    let file = f.root().join("journals").join(format!("{run}.juro"));
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
    let artifact = migrated["recovery_artifact"].as_str().unwrap();
    let canonical = f.root().join("migrations").join(format!("{artifact}.juro"));
    let legacy = canonical.with_extension("ojo");
    assert!(canonical.exists());
    assert!(!legacy.exists());
    let snapshot = fs::read(&canonical).unwrap();
    // This fixture represents an artifact produced by an older release.
    fs::rename(&canonical, &legacy).unwrap();
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
    assert!(!canonical.exists());
    assert_eq!(fs::read(&legacy).unwrap(), snapshot);
    fs::copy(&legacy, &canonical).unwrap();
    let refused = f.0.join("ambiguous-restore");
    assert!(
        !f.call(
            "restore-migration",
            &["--migration", artifact, "--out", refused.to_str().unwrap()]
        )
        .status
        .success()
    );
    assert!(!refused.exists());
    assert!(
        !f.call(
            "migrate",
            &[
                "--legacy-root",
                source.to_str().unwrap(),
                "--allow-raw-source"
            ]
        )
        .status
        .success()
    );
    fs::remove_file(&canonical).unwrap();
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

struct OwnerProcess {
    child: std::process::Child,
    socket: PathBuf,
}
impl OwnerProcess {
    fn start(f: &Fixture) -> Self {
        let root = fs::canonicalize(f.root()).unwrap();
        let socket = PathBuf::from(format!(
            "/tmp/ojo-journal-{}/{}.sock",
            unsafe { libc::geteuid() },
            &blake3::hash(root.as_os_str().as_encoded_bytes()).to_hex()[..32]
        ));
        let log = fs::File::create(f.0.join("owner.log")).unwrap();
        let child = Command::new(BIN)
            .args(["serve", "--root", root.to_str().unwrap(), "--sync-cloud"])
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .unwrap();
        let mut owner = Self { child, socket };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if let Some(status) = owner.child.try_wait().unwrap() {
                panic!(
                    "owner exited {status}: {}",
                    fs::read_to_string(f.0.join("owner.log")).unwrap()
                );
            }
            let output = f.call("status", &[]);
            if output.status.success()
                && serde_json::from_slice::<Value>(&output.stdout).unwrap()["owner"]["healthy"]
                    == true
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "owner handshake timed out"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        owner
    }
}
impl Drop for OwnerProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_file(&self.socket);
    }
}
#[test]
fn healthy_owner_reuses_handles_routes_capture_queries_and_survives_client_loss() {
    let f = Fixture::new();
    ok(f.call("init", &[]));
    let mut owner = OwnerProcess::start(&f);
    let input = f.0.join("original.txt");
    fs::write(&input, "Healthy handle exact café original\n").unwrap();
    let args = [
        "--session-id",
        "owner-session",
        "--event-id",
        "owner-event",
        "--text-file",
        input.to_str().unwrap(),
    ];
    let appended = ok(f.call("record", &args));
    let run = appended["session"].as_str().unwrap();
    assert_eq!(ok(f.call("record", &args))["stored"], 0);
    let opens = ok(f.call("status", &[]))["owner"]["verified_opens"]
        .as_u64()
        .unwrap();
    for _ in 0..5 {
        assert_eq!(
            ok(f.call("get", &["--session", run, "--id", "0"]))["input"]["text"],
            "Healthy handle exact café original\n"
        );
        assert_eq!(
            ok(f.call("timeline", &["--session", run]))["records"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }
    assert_eq!(ok(f.call("status", &[]))["owner"]["verified_opens"], opens);
    // The healthy writer owns the actual engine file, not just a PID or socket.
    let key: [u8; 32] = fs::read(f.root().join("key")).unwrap().try_into().unwrap();
    assert!(
        ojo::Database::open(
            f.root().join("journals").join(format!("{run}.juro")),
            Some(&key),
            true
        )
        .is_err()
    );
    // A response can disappear after durable acceptance. The original event ID
    // still resolves once and changed content remains a conflict on the owner.
    fs::write(&input, "changed intent").unwrap();
    assert!(!f.call("record", &args).status.success());
    fs::write(&input, "Healthy handle exact café original\n").unwrap();
    assert_eq!(ok(f.call("record", &args))["stored"], 0);
    ok(f.call("finish", &["--session", run]));
    ok(f.call("archive", &["--session", run]));
    assert_eq!(
        ok(f.call("get", &["--session", run, "--id", "0"]))["input"]["text"],
        "Healthy handle exact café original\n"
    );
    ok(f.call("archive-policy", &["--enabled", "false"]));
    ok(f.call("archive-policy", &["--enabled", "true"]));
    // Drop the local transport after sending a complete immutable write intent.
    // Retry exactly that intent after observing its durable original.
    let mut lost = ok(f.call("get", &["--session", run, "--id", "0"]));
    lost["event_id"] = json!(
        blake3::hash(b"synthetic-lost-owner-ack")
            .to_hex()
            .to_string()
    );
    let root_identity: Value =
        serde_json::from_slice(&fs::read(f.root().join("root.json")).unwrap()).unwrap();
    let intent = json!({"format":1,"root":root_identity["identity"],"action":{"operation":"Append","run":run,"request":"synthetic-lost-owner-ack","records":[lost]}});
    let encoded = serde_json::to_vec(&intent).unwrap();
    {
        let mut stream = std::os::unix::net::UnixStream::connect(&owner.socket).unwrap();
        stream
            .write_all(&(encoded.len() as u32).to_be_bytes())
            .unwrap();
        stream.write_all(&encoded).unwrap();
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if ok(f.call("timeline", &["--session", run]))["records"]
            .as_array()
            .unwrap()
            .len()
            == 2
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "lost-response write was not accepted"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    {
        use std::io::Read;
        let mut stream = std::os::unix::net::UnixStream::connect(&owner.socket).unwrap();
        stream
            .write_all(&(encoded.len() as u32).to_be_bytes())
            .unwrap();
        stream.write_all(&encoded).unwrap();
        let mut length = [0; 4];
        stream.read_exact(&mut length).unwrap();
        let mut body = vec![0; u32::from_be_bytes(length) as usize];
        stream.read_exact(&mut body).unwrap();
        let reply: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(reply["value"], 0);
        assert!(reply["error"].is_null());
    }
    let source = f.0.join("sessions");
    fs::create_dir(&source).unwrap();
    ok(f.call(
        "connect",
        &[
            "--adapter",
            "codex",
            "--source-root",
            source.to_str().unwrap(),
            "--accept-local-capture",
            "--no-service",
        ],
    ));
    fs::write(source.join("rollout-synthetic.jsonl"),format!("{}\n{}\n",json!({"type":"session_meta","payload":{"id":"owner-provider-session"}}),json!({"timestamp":"2026-10-25T00:00:00Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Automatic owner transcript synthetic context"}]}}))).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let matches = ok(f.call("search", &["--query", "Automatic owner transcript"]));
        if !matches["matches"].as_array().unwrap().is_empty() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "automatic capture timed out: {}",
            fs::read_to_string(f.0.join("owner.log")).unwrap()
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    // A killed owner leaves a stale socket. Both failed handshake and the free
    // owner lock establish offline fallback; verified file reopen keeps originals.
    owner.child.kill().unwrap();
    owner.child.wait().unwrap();
    assert!(owner.socket.exists());
    assert_eq!(
        ok(f.call("get", &["--session", run, "--id", "0"]))["input"]["text"],
        "Healthy handle exact café original\n"
    );
    let replacement = OwnerProcess::start(&f);
    assert_eq!(
        ok(f.call("timeline", &["--session", run]))["records"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    drop(replacement);
}

fn fake_agent(f: &Fixture) -> PathBuf {
    let tools = f.0.join("tools");
    fs::create_dir(&tools).unwrap();
    let script = tools.join("codex");
    fs::write(&script,r##"#!/usr/bin/env python3
import json,os,pathlib,sys
path=pathlib.Path(os.environ['JOURNAL_SETUP_AGENT_CONFIG'])
config=json.loads(path.read_text()) if path.exists() else {'unrelated':{'name':'unrelated','transport':{'type':'stdio','command':'preserve','args':[]}}}
args=sys.argv[1:]
if args==['mcp','list','--json']: print(json.dumps(list(config.values())))
elif args[:2]==['mcp','get']:
 name=args[2]
 if name not in config:print(f'No MCP server named "{name}". ',file=sys.stderr);sys.exit(1)
 item=config[name]['transport'];print('Type: stdio\nCommand: '+item['command']+'\nArgs: '+' '.join(item['args']))
elif args[:2]==['mcp','add']:
 index=args.index('--');name=args[index-1];launcher=args[index+1:]
 assert name not in config,'setup must not replace an existing entry'
 config[name]={'name':name,'enabled':True,'transport':{'type':'stdio','command':launcher[0],'args':launcher[1:],'env':None}}
 path.write_text(json.dumps(config));os.chmod(path,0o600)
else:sys.exit(2)
"##).unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
    std::os::unix::fs::symlink(&script, tools.join("claude")).unwrap();
    tools
}
#[test]
fn supported_agents_register_read_only_local_originals_and_preserve_existing_entries() {
    for adapter in ["codex", "claude-code"] {
        let f = Fixture::new();
        let tools = fake_agent(&f);
        let state = f.0.join("agent.json");
        let source = f.0.join("sessions");
        fs::create_dir(&source).unwrap();
        let connect = || {
            Command::new(BIN)
                .args([
                    "connect",
                    "--root",
                    f.root().to_str().unwrap(),
                    "--adapter",
                    adapter,
                    "--source-root",
                    source.to_str().unwrap(),
                    "--accept-local-capture",
                    "--no-service",
                    "--register-mcp",
                ])
                .env(
                    "PATH",
                    format!("{}:{}", tools.display(), std::env::var("PATH").unwrap()),
                )
                .env("JOURNAL_SETUP_AGENT_CONFIG", &state)
                .output()
                .unwrap()
        };
        let report = ok(connect());
        assert_eq!(report["mcp"]["mcp_original_verified"], true);
        assert_eq!(report["mcp"]["agent_tool_call_verified"], false);
        let saved = fs::read(&state).unwrap();
        assert_eq!(ok(connect())["mcp"]["registered"], true);
        assert_eq!(fs::read(&state).unwrap(), saved);
        let name = report["mcp"]["server"].as_str().unwrap();
        let mut changed: Value = serde_json::from_slice(&saved).unwrap();
        changed[name]["transport"]["command"] = json!("preserve-existing-command");
        fs::write(&state, serde_json::to_vec(&changed).unwrap()).unwrap();
        let existing = fs::read(&state).unwrap();
        assert!(!connect().status.success());
        assert_eq!(fs::read(&state).unwrap(), existing);
        assert_eq!(changed["unrelated"]["transport"]["command"], "preserve");
    }
}
