use super::{JournalRoot, Record, Result, capture_once, connect_local, sync_cloud};
use crate::hosted_capture;
use serde::Serialize;
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{self, BufRead, Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    time::Duration,
};

const HELP: &str = "ojo-journal — local .juro agent journals and optional scoped cloud sync\n\nCommands: serve, upgrade-service, init, connect, capture, watch, sessions, search, timeline, get, context, record, export, import, migrate, restore-migration, finish, resume, archive, archive-idle, archive-policy, cloud-connect, sync, status, mcp\n\nGlobal: --root ABSOLUTE_DIRECTORY (default ~/.local/state/ojo-journal)\nconnect --adapter codex|claude-code [--source-root DIR] [--label NAME] --accept-local-capture [--include-existing] [--api-key-file PRIVATE_FILE] [--no-service] [--no-mcp|--register-mcp]\nrecord --session-id STABLE_ID --event-id STABLE_ID [--text-file FILE] [--kind explicit|input|output|tool_call|tool_result|system]\nsearch --query TEXT [--session ID] [--limit 10]\ntimeline --session ID [--from 0] [--limit 100]\nget/context --session ID --id SEQUENCE\nexport --session ID (JSONL on stdout); import --file JSONL\nfinish/resume/archive --session ID; archive-policy --enabled true|false\ncloud-connect --api-key-file PRIVATE_FILE [--base-url https://oojuro.com]\nsync [--max-batches 1000]; local recording needs no cloud account.\n";
struct Args {
    command: String,
    values: BTreeMap<String, String>,
    flags: BTreeSet<String>,
}
impl Args {
    fn parse() -> Result<Self> {
        let mut all = std::env::args().skip(1);
        let command = all.next().unwrap_or_else(|| "help".into());
        let mut args = Self {
            command,
            values: BTreeMap::new(),
            flags: BTreeSet::new(),
        };
        while let Some(flag) = all.next() {
            if !flag.starts_with("--")
                || args.values.contains_key(&flag)
                || args.flags.contains(&flag)
            {
                return Err("invalid_arguments");
            }
            if [
                "--plaintext",
                "--accept-local-capture",
                "--include-existing",
                "--no-service",
                "--once",
                "--sync-cloud",
                "--allow-raw-source",
                "--no-mcp",
                "--register-mcp",
            ]
            .contains(&flag.as_str())
            {
                args.flags.insert(flag);
            } else {
                let value = all.next().ok_or("missing_option_value")?;
                if value.len() > 65536 {
                    return Err("argument_bound");
                }
                args.values.insert(flag, value);
            }
        }
        Ok(args)
    }
    fn val(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }
    fn req(&self, name: &str) -> Result<&str> {
        self.val(name).ok_or("missing_required_option")
    }
    fn num<T: std::str::FromStr>(&self, name: &str, default: T) -> Result<T> {
        self.val(name)
            .map(|s| s.parse().map_err(|_| "invalid_number"))
            .unwrap_or(Ok(default))
    }
    fn only(&self, values: &[&str], flags: &[&str]) -> Result<()> {
        if self
            .values
            .keys()
            .any(|k| k != "--root" && !values.contains(&k.as_str()))
            || self.flags.iter().any(|k| !flags.contains(&k.as_str()))
        {
            return Err("unknown_option");
        }
        Ok(())
    }
}
fn emit(value: &impl Serialize) -> Result<()> {
    let mut output = io::stdout().lock();
    serde_json::to_writer(&mut output, value).map_err(|_| "output_failed")?;
    output.write_all(b"\n").map_err(|_| "output_failed")
}
fn default_root() -> Result<PathBuf> {
    Ok(
        PathBuf::from(std::env::var_os("HOME").ok_or("home_directory_unavailable")?)
            .join(".local/state/ojo-journal"),
    )
}
fn absolute(path: &str) -> Result<PathBuf> {
    std::path::absolute(path).map_err(|_| "invalid_path")
}
fn read_text(path: Option<&str>) -> Result<String> {
    let mut bytes = Vec::new();
    match path {
        Some(path) => {
            let f = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(path)
                .map_err(|_| "input_unavailable")?;
            if !f.metadata().map_err(|_| "input_unavailable")?.is_file() {
                return Err("regular_input_required");
            }
            f.take((super::store::MAX_TEXT + 1) as u64)
                .read_to_end(&mut bytes)
                .map_err(|_| "input_unavailable")?;
        }
        None => {
            io::stdin()
                .take((super::store::MAX_TEXT + 1) as u64)
                .read_to_end(&mut bytes)
                .map_err(|_| "input_unavailable")?;
        }
    }
    if bytes.len() > super::store::MAX_TEXT {
        return Err("original_text_bound");
    }
    String::from_utf8(bytes).map_err(|_| "utf8_input_required")
}
pub fn run() -> Result<()> {
    let args = Args::parse()?;
    if matches!(args.command.as_str(), "help" | "--help" | "-h") {
        print!("{HELP}");
        return Ok(());
    }
    if matches!(args.command.as_str(), "version" | "--version") {
        println!("ojo-journal {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let path = match args.val("--root") {
        Some(path) => absolute(path)?,
        None => default_root()?,
    };
    if args.command == "serve" {
        args.only(&[], &["--sync-cloud"])?;
        return super::owner::serve(&path, args.flags.contains("--sync-cloud"));
    }
    if args.command == "init" {
        args.only(&[], &["--plaintext"])?;
        let root = JournalRoot::init(&path, !args.flags.contains("--plaintext"))?;
        return emit(
            &json!({"storage":"ojo","encrypted":root.encrypted(),"identity":root.identity(),"root":path}),
        );
    }
    if args.command == "connect" {
        args.only(
            &[
                "--adapter",
                "--source-root",
                "--label",
                "--api-key-file",
                "--base-url",
            ],
            &[
                "--accept-local-capture",
                "--include-existing",
                "--no-service",
                "--no-mcp",
                "--register-mcp",
            ],
        )?;
        if !args.flags.contains("--accept-local-capture") {
            return Err("explicit_local_capture_consent_required");
        }
        let root = if path.exists() {
            JournalRoot::open(&path)?
        } else {
            JournalRoot::init(&path, true)?
        };
        let adapter = args.req("--adapter")?;
        let home = PathBuf::from(std::env::var_os("HOME").ok_or("home_directory_unavailable")?);
        let source = args
            .val("--source-root")
            .map(absolute)
            .transpose()?
            .unwrap_or_else(|| {
                home.join(if adapter == "codex" {
                    ".codex/sessions"
                } else {
                    ".claude/projects"
                })
            });
        let report = super::capture::setup_retry(|| {
            connect_local(
                &root,
                &source,
                adapter,
                args.val("--label").unwrap_or(adapter),
                args.flags.contains("--include-existing"),
            )
        })?;
        if let Some(file) = args.val("--api-key-file") {
            hosted_capture::ensure_journal_connection(
                &path.join("cloud.json"),
                args.val("--base-url").unwrap_or("https://oojuro.com"),
                &absolute(file)?,
                args.val("--label").unwrap_or(adapter),
            )?;
        }
        let cloud_scope = if path.join("cloud.json").exists() {
            Some(super::sync::initialize(
                &root,
                &path.join("cloud.json"),
                args.flags.contains("--include-existing"),
            )?)
        } else {
            None
        };
        let mut verification = super::capture::setup_retry(|| {
            super::capture::verify_pipeline(&root, adapter, cloud_scope.as_deref())
        })?;
        let cloud_result = if cloud_scope.is_some() {
            sync_cloud(&root, &path.join("cloud.json"), 8).map(|_| ())
        } else {
            Ok(())
        };
        let service = if args.flags.contains("--no-service") {
            false
        } else {
            install_service(&root, adapter, false)?;
            // A running systemd unit alone does not establish that capture owns
            // a healthy database. Probe the owner and re-read the accepted parser fixture.
            let owner_root = JournalRoot::open(&path)?;
            verification = super::capture::setup_retry(|| {
                super::capture::verify_pipeline(&owner_root, adapter, cloud_scope.as_deref())
            })?;
            true
        };
        cloud_result?;
        if args.flags.contains("--no-mcp") && args.flags.contains("--register-mcp") {
            return Err("conflicting_mcp_options");
        }
        let mcp = if args.flags.contains("--register-mcp")
            || (service && !args.flags.contains("--no-mcp"))
        {
            let binary = if service {
                root.path.join("bin/ojo-journal")
            } else {
                std::env::current_exe().map_err(|_| "current_binary_unavailable")?
            };
            Some(super::setup::register(
                &JournalRoot::open(&path)?,
                adapter,
                &binary,
                &verification,
            )?)
        } else {
            None
        };
        let cloud_confirmed = cloud_scope
            .as_ref()
            .map(|scope| {
                super::sync::confirmed(&root, scope, &verification.session, verification.sequence)
            })
            .transpose()?
            .unwrap_or(false);
        if cloud_scope.is_some() && !cloud_confirmed {
            return Err("setup_cloud_delivery_unconfirmed; local capture remains configured");
        }
        return emit(
            &json!({"connected":true,"storage":"ojo","capture":report,"verification":verification,"cloud_confirmed":cloud_confirmed,"service_started":service,"mcp":mcp,"existing_history_selected":args.flags.contains("--include-existing"),"next":"New complete transcript lines are captured into one .juro per provider session; use status to inspect durable records and sync confirmations."}),
        );
    }
    let mut root = JournalRoot::open(&path)?;
    match args.command.as_str() {
        "upgrade-service" => {
            args.only(&["--adapter"], &[])?;
            install_service(&root, args.val("--adapter").unwrap_or("codex"), true)?;
            emit(&json!({"service_upgraded":true,"owner_verified":true}))
        }
        "capture" => {
            args.only(&["--adapter"], &["--once"])?;
            emit(&capture_once(&root, args.req("--adapter")?)?)
        }
        "watch" => {
            args.only(&["--adapter"], &["--sync-cloud"])?;
            watch(
                root,
                args.req("--adapter")?,
                args.flags.contains("--sync-cloud"),
            )
        }
        "sessions" | "status" => {
            args.only(&[], &[])?;
            emit(
                &json!({"storage":"ojo","encrypted":root.encrypted(),"sessions":root.sessions()?,"cloud":super::sync::status(&root)?,"owner":root.owner_status()?}),
            )
        }
        "timeline" => {
            args.only(&["--session", "--from", "--limit"], &[])?;
            emit(&root.page(
                args.req("--session")?,
                args.num("--from", 0)?,
                args.num("--limit", 100)?,
            )?)
        }
        "get" => {
            args.only(&["--session", "--id"], &[])?;
            emit(&root.get(args.req("--session")?, args.num("--id", 0)?)?)
        }
        "context" => {
            args.only(&["--session", "--id", "--before", "--after"], &[])?;
            let id: u64 = args.num("--id", 0)?;
            let before: u64 = args.num("--before", 3)?;
            let after: usize = args.num("--after", 3)?;
            if before > 49 || after > 49 {
                return Err("context_bound");
            }
            emit(
                &root.page(
                    args.req("--session")?,
                    id.saturating_sub(before),
                    id.saturating_add(after as u64)
                        .saturating_add(1)
                        .saturating_sub(id.saturating_sub(before)) as usize,
                )?,
            )
        }
        "search" => {
            args.only(&["--session", "--query", "--limit"], &[])?;
            let limit = args.num("--limit", 10)?;
            let sessions = if let Some(run) = args.val("--session") {
                vec![root.manifest(run)?]
            } else {
                root.sessions()?
            };
            let mut records = Vec::new();
            let mut bytes = 0usize;
            for session in sessions {
                for record in root.search(&session.run_id, args.req("--query")?, limit)? {
                    bytes += record.input.text.len();
                    if bytes > super::store::MAX_TEXT {
                        return Err("search_output_bound");
                    }
                    records.push(record);
                    if records.len() == limit {
                        break;
                    }
                }
                if records.len() == limit {
                    break;
                }
            }
            emit(
                &json!({"matches":records,"query_semantics":"case-insensitive AND substrings; newest matching records per session"}),
            )
        }
        "record" => {
            args.only(
                &[
                    "--session-id",
                    "--event-id",
                    "--text-file",
                    "--kind",
                    "--label",
                    "--time-us",
                ],
                &[],
            )?;
            use oojuro_engine::record::{Input, Kind};
            let session = args.req("--session-id")?;
            let event = args.req("--event-id")?;
            if session.is_empty() || session.len() > 512 || event.is_empty() || event.len() > 512 {
                return Err("identity_bound");
            }
            let run = blake3::hash(
                format!(
                    "ojo-journal/manual-session/v1:{}:{session}",
                    root.identity()
                )
                .as_bytes(),
            )
            .to_hex()
            .to_string();
            let id = blake3::hash(format!("ojo-journal/manual-event/v1:{run}:{event}").as_bytes())
                .to_hex()
                .to_string();
            let kind = match args.val("--kind").unwrap_or("explicit") {
                "explicit" => Kind::Explicit,
                "input" => Kind::Input,
                "output" => Kind::Output,
                "tool_call" => Kind::ToolCall,
                "tool_result" => Kind::ToolResult,
                "system" => Kind::System,
                _ => return Err("unsupported_kind"),
            };
            let text = read_text(args.val("--text-file"))?;
            root.ensure_session(
                &run,
                root.identity(),
                "ojo-journal/direct-v1",
                args.val("--label").unwrap_or("Manual journal"),
            )?;
            let record = Record {
                sequence: 0,
                event_id: id.clone(),
                run_id: run.clone(),
                adapter: "ojo-journal/direct-v1".into(),
                input: Input {
                    agent_id: root.identity().into(),
                    parent_id: None,
                    valid_at: args.num("--time-us", 0)?,
                    kind,
                    tool: None,
                    status: None,
                    exit: None,
                    key: None,
                    text,
                    attrs: BTreeMap::new(),
                    indexed: kind != Kind::System,
                },
            };
            let stored = root.append(&run, &id, &[record])?;
            emit(&json!({"session":run,"event_id":id,"stored":stored,"durable":true}))
        }
        "export" => {
            args.only(&["--session"], &[])?;
            let run = args.req("--session")?;
            emit(&json!({"format":1,"manifest":root.manifest(run)?}))?;
            root.export(run, &mut io::stdout().lock())?;
            Ok(())
        }
        "import" => {
            args.only(&["--file"], &[])?;
            import(&root, Path::new(args.req("--file")?))
        }
        "migrate" => {
            args.only(&["--legacy-root"], &["--allow-raw-source"])?;
            if !args.flags.contains("--allow-raw-source") {
                return Err("explicit_legacy_source_consent_required");
            }
            emit(&super::migrate_legacy(
                &root,
                &absolute(args.req("--legacy-root")?)?,
            )?)
        }
        "restore-migration" => {
            args.only(&["--migration", "--out"], &[])?;
            emit(
                &json!({"restored_files":super::restore_migration(&root,args.req("--migration")?,&absolute(args.req("--out")?)?)?}),
            )
        }
        "finish" | "resume" => {
            args.only(&["--session"], &[])?;
            emit(&root.finish(args.req("--session")?, args.command == "finish")?)
        }
        "archive" => {
            args.only(&["--session"], &[])?;
            emit(&root.archive(args.req("--session")?)?)
        }
        "archive-idle" => {
            args.only(&[], &[])?;
            emit(&json!({"archived":root.archive_idle(super::store::now())?}))
        }
        "archive-policy" => {
            args.only(&["--enabled"], &[])?;
            let enabled: bool = args
                .req("--enabled")?
                .parse()
                .map_err(|_| "invalid_archive_policy")?;
            root.set_automatic_archive(enabled)?;
            emit(
                &json!({"automatic_archive":enabled,"idle_hours":24,"requires_finished_session":true}),
            )
        }
        "cloud-connect" => {
            args.only(
                &["--api-key-file", "--base-url", "--label"],
                &["--include-existing"],
            )?;
            let info = hosted_capture::ensure_journal_connection(
                &path.join("cloud.json"),
                args.val("--base-url").unwrap_or("https://oojuro.com"),
                &absolute(args.req("--api-key-file")?)?,
                args.val("--label").unwrap_or("ojo-journal"),
            )?;
            super::sync::initialize(
                &root,
                &path.join("cloud.json"),
                args.flags.contains("--include-existing"),
            )?;
            emit(&info)
        }
        "sync" => {
            args.only(&["--max-batches"], &[])?;
            emit(&sync_cloud(
                &root,
                &path.join("cloud.json"),
                args.num("--max-batches", 1000)?,
            )?)
        }
        "mcp" => {
            args.only(&[], &[])?;
            mcp(&root)
        }
        _ => Err("unknown_command; run ojo-journal help"),
    }
}
fn import(root: &JournalRoot, path: &Path) -> Result<()> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| "import_unavailable")?;
    if !file.metadata().map_err(|_| "import_unavailable")?.is_file() {
        return Err("regular_import_file_required");
    }
    let mut reader = io::BufReader::new(file);
    let mut manifest: Option<super::Manifest> = None;
    let mut imported = 0u64;
    let mut duplicate = 0u64;
    loop {
        let mut line = Vec::new();
        reader
            .by_ref()
            .take((100 << 20) + 1)
            .read_until(b'\n', &mut line)
            .map_err(|_| "import_unavailable")?;
        if line.is_empty() {
            break;
        }
        if line.len() > 100 << 20 {
            return Err("import_record_bound");
        }
        let value: serde_json::Value =
            serde_json::from_slice(&line).map_err(|_| "invalid_import_json")?;
        if value.get("manifest").is_some() {
            if manifest.is_some() || imported != 0 {
                return Err("invalid_import_header");
            }
            manifest = Some(
                serde_json::from_value(value["manifest"].clone())
                    .map_err(|_| "invalid_import_header")?,
            );
            continue;
        }
        let record: Record = serde_json::from_value(value).map_err(|_| "invalid_import_record")?;
        let label = manifest
            .as_ref()
            .filter(|m| m.run_id == record.run_id)
            .map(|m| m.label.as_str())
            .unwrap_or("Imported journal");
        root.ensure_session(
            &record.run_id,
            &record.input.agent_id,
            &record.adapter,
            label,
        )?;
        let request = blake3::hash(
            format!(
                "ojo-journal/import/v1:{}:{}",
                record.run_id, record.event_id
            )
            .as_bytes(),
        )
        .to_hex()
        .to_string();
        let added = root.append(&record.run_id, &request, std::slice::from_ref(&record))?;
        imported += added;
        duplicate += 1 - added;
    }
    emit(&json!({"imported":imported,"deduplicated":duplicate,"source_untouched":true}))
}
fn unit_quote(s: &str) -> Result<String> {
    if s.chars().any(char::is_control) {
        return Err("invalid_service_path");
    }
    Ok(format!(
        "\"{}\"",
        s.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%")
    ))
}
fn install_service(root: &JournalRoot, adapter: &str, upgrade: bool) -> Result<()> {
    oojuro_capture::Format::from_name(adapter).ok_or("unsupported_adapter")?;
    let binary = std::env::current_exe().map_err(|_| "current_binary_unavailable")?;
    let bin = root.path.join("bin");
    if !bin.exists() {
        hosted_capture::directory(&bin)?;
    }
    let installed = bin.join("ojo-journal");
    let bytes = fs::read(binary).map_err(|_| "current_binary_unavailable")?;
    if !installed.exists() {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o700)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&installed)
            .map_err(|_| "service_binary_install_failed")?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| "service_binary_install_failed")?;
    } else {
        let info = fs::symlink_metadata(&installed).map_err(|_| "service_binary_unavailable")?;
        if !info.is_file()
            || info.uid() != unsafe { libc::geteuid() }
            || info.mode() & 0o777 != 0o700
        {
            return Err("private_service_binary_required");
        }
        if fs::read(&installed).map_err(|_| "service_binary_unavailable")? != bytes {
            if !upgrade {
                return Err(
                    "existing_service_binary_requires_explicit_upgrade; use ojo-journal upgrade-service",
                );
            }
            let temporary = bin.join(format!(".upgrade-{}", hosted_capture::random_id()?));
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o700)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&temporary)
                .map_err(|_| "service_binary_install_failed")?;
            file.write_all(&bytes)
                .and_then(|_| file.sync_all())
                .map_err(|_| "service_binary_install_failed")?;
            fs::rename(&temporary, &installed).map_err(|_| "service_upgrade_outcome_unknown")?;
        }
    }
    File::open(&bin)
        .and_then(|f| f.sync_all())
        .map_err(|_| "service_binary_install_failed")?;
    let home = PathBuf::from(std::env::var_os("HOME").ok_or("home_directory_unavailable")?);
    let units = home.join(".config/systemd/user");
    fs::create_dir_all(&units).map_err(|_| "service_directory_unavailable")?;
    let unit_name = format!("ojo-journal-{}.service", &root.identity()[..16]);
    let unit = units.join(&unit_name);
    let body = format!(
        "# Managed by ojo-journal root {}\n[Unit]\nDescription=ojo-journal local session owner\n[Service]\nExecStart={} serve --root {} --sync-cloud\nRestart=on-failure\nRestartSec=3\n[Install]\nWantedBy=default.target\n",
        root.identity(),
        unit_quote(installed.to_str().ok_or("invalid_service_path")?)?,
        unit_quote(root.path.to_str().ok_or("invalid_service_path")?)?
    );
    if unit.exists() {
        let existing = read_private_unit(&unit)?;
        if existing != body.as_bytes() {
            if !upgrade
                || !existing.starts_with(
                    format!("# Managed by ojo-journal root {}\n", root.identity()).as_bytes(),
                )
            {
                return Err("existing_service_unit_requires_explicit_upgrade");
            }
            let temporary = units.join(format!(".upgrade-{}", hosted_capture::random_id()?));
            hosted_capture::create_private(&temporary, body.as_bytes())?;
            fs::rename(&temporary, &unit).map_err(|_| "service_upgrade_outcome_unknown")?;
        }
    } else {
        hosted_capture::create_private(&unit, body.as_bytes())?;
    }
    // Upgrade only the exact former unit for this private root. An unrelated
    // service using the old per-adapter name is never stopped or rewritten.
    for old_adapter in ["codex", "claude-code"] {
        let old_name = format!("ojo-journal-{old_adapter}.service");
        let old = units.join(&old_name);
        if old.exists() {
            let expected = format!(
                "# Managed by ojo-journal\n[Unit]\nDescription=ojo-journal {old_adapter} local capture\n[Service]\nExecStart={} watch --root {} --adapter {old_adapter} --sync-cloud\nRestart=on-failure\nRestartSec=3\n[Install]\nWantedBy=default.target\n",
                unit_quote(installed.to_str().ok_or("invalid_service_path")?)?,
                unit_quote(root.path.to_str().ok_or("invalid_service_path")?)?
            );
            if read_private_unit(&old)? == expected.as_bytes() {
                if !upgrade {
                    return Err(
                        "existing_service_unit_requires_explicit_upgrade; use ojo-journal upgrade-service",
                    );
                }
                if !std::process::Command::new("systemctl")
                    .args(["--user", "disable", "--now", &old_name])
                    .status()
                    .map_err(|_| "systemd_user_service_unavailable")?
                    .success()
                {
                    return Err("old_service_stop_unconfirmed");
                }
                fs::remove_file(&old).map_err(|_| "old_service_unit_unavailable")?;
            }
        }
    }
    File::open(&units)
        .and_then(|file| file.sync_all())
        .map_err(|_| "service_unit_sync_failed")?;
    for arguments in [
        vec!["--user", "daemon-reload"],
        vec!["--user", "enable", "--now", unit_name.as_str()],
        vec!["--user", "restart", unit_name.as_str()],
    ] {
        if !std::process::Command::new("systemctl")
            .args(arguments)
            .status()
            .map_err(|_| "systemd_user_service_unavailable")?
            .success()
        {
            return Err("systemd_user_service_unavailable; use ojo-journal watch in foreground");
        }
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        match super::owner::available(root) {
            Ok(true) => break,
            _ if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50))
            }
            _ => {
                return Err(
                    "service_started_but_owner_unconfirmed; inspect the managed service log",
                );
            }
        }
    }
    Ok(())
}
fn read_private_unit(path: &Path) -> Result<Vec<u8>> {
    crate::hosted::read_private(path, 16 << 10).map_err(|_| "private_service_unit_required")
}
fn watch(root: JournalRoot, adapter: &str, cloud: bool) -> Result<()> {
    oojuro_capture::Format::from_name(adapter).ok_or("unsupported_adapter")?;
    super::owner::serve(&root.path, cloud)
}
fn mcp(root: &JournalRoot) -> Result<()> {
    let mut input = io::stdin().lock();
    loop {
        let mut line = Vec::new();
        input
            .by_ref()
            .take(65537)
            .read_until(b'\n', &mut line)
            .map_err(|_| "mcp_input_failed")?;
        if line.is_empty() {
            break;
        }
        if line.len() > 65536 {
            return Err("mcp_request_bound");
        }
        let request: serde_json::Value = match serde_json::from_slice(&line) {
            Ok(value) => value,
            Err(_) => {
                emit(
                    &json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"invalid_mcp_json"}}),
                )?;
                continue;
            }
        };
        let Some(id) = request.get("id") else {
            continue;
        };
        let result = (|| -> Result<serde_json::Value> {
            match request["method"].as_str().unwrap_or("") {
                "initialize" => Ok(
                    json!({"protocolVersion":"2025-11-25","capabilities":{"tools":{}},"serverInfo":{"name":"ojo-journal","version":env!("CARGO_PKG_VERSION")}}),
                ),
                "ping" => Ok(json!({})),
                "tools/list" => Ok(json!({"tools":[
                    {"annotations":{"readOnlyHint":true,"destructiveHint":false},"name":"sessions","description":"List local .juro session journals","inputSchema":{"type":"object","properties":{},"additionalProperties":false}},
                    {"annotations":{"readOnlyHint":true,"destructiveHint":false},"name":"search","description":"Search recorded originals in one local session","inputSchema":{"type":"object","properties":{"session":{"type":"string"},"query":{"type":"string"}},"required":["session","query"],"additionalProperties":false}},
                    {"annotations":{"readOnlyHint":true,"destructiveHint":false},"name":"get","description":"Read the exact original and its session metadata","inputSchema":{"type":"object","properties":{"session":{"type":"string"},"id":{"type":"integer","minimum":0}},"required":["session","id"],"additionalProperties":false}}
                ]})),
                "tools/call" => {
                    let p = &request["params"];
                    let a = &p["arguments"];
                    match p["name"].as_str().unwrap_or("") {
                        "sessions" => {
                            serde_json::to_value(root.sessions()?).map_err(|_| "mcp_output_failed")
                        }
                        "search" => serde_json::to_value(root.search(
                            a["session"].as_str().ok_or("invalid_mcp_arguments")?,
                            a["query"].as_str().ok_or("invalid_mcp_arguments")?,
                            10,
                        )?)
                        .map_err(|_| "mcp_output_failed"),
                        "get" => serde_json::to_value(root.get(
                            a["session"].as_str().ok_or("invalid_mcp_arguments")?,
                            a["id"].as_u64().ok_or("invalid_mcp_arguments")?,
                        )?)
                        .map_err(|_| "mcp_output_failed"),
                        _ => Err("unknown_mcp_tool"),
                    }
                    .map(|value| json!({"content":[{"type":"text","text":value.to_string()}]}))
                }
                _ => Err("unsupported_mcp_method"),
            }
        })();
        match result {
            Ok(result) => emit(&json!({"jsonrpc":"2.0","id":id,"result":result}))?,
            Err(error) => {
                emit(&json!({"jsonrpc":"2.0","id":id,"error":{"code":-32602,"message":error}}))?
            }
        }
    }
    Ok(())
}
