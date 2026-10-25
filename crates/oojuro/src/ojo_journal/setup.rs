//! Agent registration uses the supported CLI and preserves existing entries.
use super::{JournalRoot, Result, Verification};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    os::{fd::AsRawFd, unix::process::CommandExt},
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
#[derive(Serialize)]
pub(crate) struct Registration {
    pub server: String,
    pub registered: bool,
    pub mcp_original_verified: bool,
    pub agent_tool_call_verified: bool,
}
fn bounded_read(mut pipe: impl Read + AsRawFd) -> std::io::Result<Vec<u8>> {
    let fd = pipe.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut result = Vec::new();
    let mut bytes = [0u8; 8192];
    loop {
        match pipe.read(&mut bytes) {
            Ok(0) => return Ok(result),
            Ok(n) => {
                result.extend_from_slice(&bytes[..n]);
                if result.len() > 1 << 20 {
                    return Ok(result);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(20))
            }
            Err(error) => return Err(error),
        }
        if Instant::now() >= deadline {
            return Err(std::io::Error::from(std::io::ErrorKind::TimedOut));
        }
    }
}
fn process(
    program: &Path,
    args: &[String],
    input: Option<Vec<u8>>,
) -> Result<(bool, Vec<u8>, Vec<u8>)> {
    let mut command = Command::new(program);
    command
        .args(args)
        .env_remove("OOJURO_API_KEY")
        .env_remove("OOJURO_PROJECT_READ_KEY")
        .process_group(0)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|_| "agent_command_unavailable")?;
    let pid = child.id();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let out = std::thread::spawn(move || bounded_read(stdout));
    let err = std::thread::spawn(move || bounded_read(stderr));
    let writer = if let Some(input) = input {
        let mut stdin = child.stdin.take().unwrap();
        Some(std::thread::spawn(move || stdin.write_all(&input)))
    } else {
        None
    };
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        match child.try_wait().map_err(|_| "agent_command_unavailable")? {
            Some(status) => break status,
            None if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            None => {
                unsafe {
                    libc::kill(-(pid as i32), libc::SIGKILL);
                };
                let _ = child.wait();
                return Err("agent_command_timed_out");
            }
        }
    };
    if let Some(writer) = writer {
        writer
            .join()
            .map_err(|_| "agent_input_unavailable")?
            .map_err(|_| "agent_input_unavailable")?;
    }
    let stdout = out
        .join()
        .map_err(|_| "agent_output_unavailable")?
        .map_err(|_| "agent_output_unavailable")?;
    let stderr = err
        .join()
        .map_err(|_| "agent_output_unavailable")?
        .map_err(|_| "agent_output_unavailable")?;
    if stdout.len() > 1 << 20 || stderr.len() > 1 << 20 {
        return Err("agent_output_bound");
    }
    Ok((status.success(), stdout, stderr))
}
fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| (*s).into()).collect()
}
pub(crate) fn register(
    root: &JournalRoot,
    adapter: &str,
    binary: &Path,
    verification: &Verification,
) -> Result<Registration> {
    register_inner(root, adapter, binary, verification, false)
}
fn register_inner(
    root: &JournalRoot,
    adapter: &str,
    binary: &Path,
    verification: &Verification,
    added: bool,
) -> Result<Registration> {
    let program = Path::new(if adapter == "codex" {
        "codex"
    } else {
        "claude"
    });
    let name = format!("ojo-journal-{}", &root.identity()[..16]);
    let launcher = vec![
        binary.to_str().ok_or("invalid_mcp_launcher")?.to_string(),
        "mcp".into(),
        "--root".into(),
        root.path.to_str().ok_or("invalid_mcp_launcher")?.into(),
    ];
    let configured = if adapter == "codex" {
        let (success, stdout, _) = process(program, &strings(&["mcp", "list", "--json"]), None)?;
        if !success {
            return Err("agent_mcp_inventory_unavailable");
        }
        let inventory: Vec<Value> =
            serde_json::from_slice(&stdout).map_err(|_| "unrecognized_agent_mcp_inventory")?;
        let mut found = false;
        for server in inventory {
            let Some(server_name) = server["name"].as_str() else {
                return Err("unrecognized_agent_mcp_inventory");
            };
            if server_name == name {
                let transport = &server["transport"];
                if server["enabled"] == false
                    || transport["type"] != "stdio"
                    || transport["command"] != launcher[0]
                    || transport["args"] != json!(&launcher[1..])
                    || transport
                        .get("env")
                        .filter(|value| !value.is_null())
                        .is_some_and(|value| value.as_object().is_none_or(|env| !env.is_empty()))
                {
                    return Err(
                        "existing_agent_mcp_entry_conflicts; existing configuration preserved",
                    );
                }
                found = true;
            }
        }
        found
    } else {
        let (success, stdout, stderr) = process(program, &strings(&["mcp", "get", &name]), None)?;
        if success {
            let text = String::from_utf8(stdout).map_err(|_| "unrecognized_agent_mcp_inventory")?;
            if !text
                .lines()
                .any(|s| s.trim() == format!("Command: {}", launcher[0]))
                || !text
                    .lines()
                    .any(|s| s.trim() == format!("Args: {}", launcher[1..].join(" ")))
            {
                return Err("existing_agent_mcp_entry_conflicts; existing configuration preserved");
            }
            true
        } else {
            if !stdout.is_empty()
                || !stderr.starts_with(format!("No MCP server named \"{name}\".").as_bytes())
            {
                return Err("unrecognized_agent_mcp_inventory");
            }
            false
        }
    };
    if !configured {
        if added {
            return Err("agent_mcp_registration_unconfirmed; inspect agent configuration");
        }
        let mut args = if adapter == "codex" {
            strings(&["mcp", "add", &name, "--"])
        } else {
            strings(&[
                "mcp",
                "add",
                "--transport",
                "stdio",
                "--scope",
                "user",
                &name,
                "--",
            ])
        };
        args.extend(launcher.clone());
        if !process(program, &args, None)?.0 {
            return Err("agent_mcp_registration_unconfirmed; inspect agent configuration");
        }
        return register_inner(root, adapter, binary, verification, true);
    }
    let frames = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"ojo-journal-setup","version":"1"}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"get","arguments":{"session":verification.session,"id":verification.sequence}}}),
    ];
    let mut input = Vec::new();
    for frame in frames {
        serde_json::to_writer(&mut input, &frame).map_err(|_| "invalid_mcp_verification")?;
        input.push(b'\n');
    }
    let (success, stdout, _) = process(binary, &launcher[1..], Some(input))?;
    if !success {
        return Err("mcp_verification_unconfirmed");
    }
    let expected = root.get(&verification.session, verification.sequence)?;
    let mut verified = false;
    for line in stdout.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
        let response: Value =
            serde_json::from_slice(line).map_err(|_| "invalid_mcp_verification")?;
        if response["id"] == 2 {
            let actual: Value = serde_json::from_str(
                response["result"]["content"][0]["text"]
                    .as_str()
                    .ok_or("mcp_original_unconfirmed")?,
            )
            .map_err(|_| "mcp_original_unconfirmed")?;
            verified = actual == json!(expected);
        }
    }
    if !verified {
        return Err("mcp_original_unconfirmed");
    }
    Ok(Registration {
        server: name,
        registered: true,
        mcp_original_verified: true,
        agent_tool_call_verified: false,
    })
}
