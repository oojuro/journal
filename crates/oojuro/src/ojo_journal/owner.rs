//! Private local transport for the process that owns healthy session handles.
//! Network uploads and transcript parsing run outside the database mutex.
use super::{JournalRoot, Record, Result};
use crate::{hosted::read_private, hosted_capture};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::{
            fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
            net::{UnixListener, UnixStream},
        },
    },
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

const MAX_MESSAGE: usize = 128 << 20;
#[derive(Serialize, Deserialize)]
#[serde(tag = "operation", deny_unknown_fields)]
pub(crate) enum Action {
    Hello,
    Ensure {
        run: String,
        agent: String,
        adapter: String,
        label: String,
    },
    Append {
        run: String,
        request: String,
        records: Vec<Record>,
    },
    Manifest {
        run: String,
    },
    Sessions,
    Get {
        run: String,
        sequence: u64,
    },
    Page {
        run: String,
        start: u64,
        limit: usize,
    },
    Search {
        run: String,
        query: String,
        limit: usize,
    },
    Finish {
        run: String,
        finished: bool,
    },
    Archive {
        run: String,
    },
    MigrateSession {
        run: String,
    },
    ArchiveIdle {
        time_us: i64,
    },
    ArchivePolicy {
        enabled: bool,
    },
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    format: u32,
    root: String,
    action: Action,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Response {
    format: u32,
    root: String,
    value: Value,
    error: Option<String>,
}
pub(crate) fn socket(root: &JournalRoot) -> Result<PathBuf> {
    let directory = PathBuf::from(format!("/tmp/ojo-journal-{}", unsafe { libc::geteuid() }));
    let hash = blake3::hash(root.path.as_os_str().as_encoded_bytes()).to_hex();
    Ok(directory.join(format!("{}.sock", &hash[..32])))
}
fn validate_socket(path: &std::path::Path) -> Result<()> {
    use std::os::unix::fs::FileTypeExt;
    let parent = path.parent().ok_or("invalid_owner_socket")?;
    let p = fs::symlink_metadata(parent).map_err(|_| "owner_directory_unavailable")?;
    if !p.is_dir() || p.uid() != unsafe { libc::geteuid() } || p.mode() & 0o777 != 0o700 {
        return Err("private_owner_directory_required");
    }
    let m = fs::symlink_metadata(path).map_err(|_| "owner_unavailable")?;
    if !m.file_type().is_socket()
        || m.uid() != unsafe { libc::geteuid() }
        || m.mode() & 0o777 != 0o600
    {
        return Err("private_owner_socket_required");
    }
    Ok(())
}
fn peer(stream: &UnixStream) -> Result<()> {
    let mut credentials: libc::ucred = unsafe { std::mem::zeroed() };
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut length,
        )
    };
    if rc != 0
        || length as usize != std::mem::size_of::<libc::ucred>()
        || credentials.uid != unsafe { libc::geteuid() }
    {
        return Err("owner_peer_refused");
    }
    Ok(())
}
fn connection_error(error: i32) -> &'static str {
    match error {
        libc::ECONNREFUSED => "owner_connection_refused",
        libc::ENOENT => "owner_endpoint_absent",
        libc::EAGAIN => "owner_backlog_busy",
        _ => "owner_transport_unavailable",
    }
}
fn connect(path: &std::path::Path) -> Result<UnixStream> {
    let raw = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
            0,
        )
    };
    if raw < 0 {
        return Err("owner_transport_unavailable");
    }
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    let bytes = path.as_os_str().as_encoded_bytes();
    if bytes.len() >= address.sun_path.len() {
        return Err("owner_socket_path_bound");
    }
    for (target, source) in address.sun_path.iter_mut().zip(bytes) {
        *target = *source as libc::c_char;
    }
    let rc = unsafe {
        libc::connect(
            fd.as_raw_fd(),
            (&address as *const libc::sockaddr_un).cast(),
            (std::mem::offset_of!(libc::sockaddr_un, sun_path) + bytes.len() + 1)
                as libc::socklen_t,
        )
    };
    if rc != 0 {
        let error = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
        if error != libc::EINPROGRESS {
            return Err(connection_error(error));
        }
        let mut wait = libc::pollfd {
            fd: fd.as_raw_fd(),
            events: libc::POLLOUT,
            revents: 0,
        };
        if unsafe { libc::poll(&mut wait, 1, 2000) } != 1 {
            return Err("owner_unavailable");
        }
        let mut error = 0i32;
        let mut length = std::mem::size_of::<i32>() as libc::socklen_t;
        if unsafe {
            libc::getsockopt(
                fd.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_ERROR,
                (&mut error as *mut i32).cast(),
                &mut length,
            )
        } != 0
        {
            return Err("owner_transport_unavailable");
        }
        if error != 0 {
            return Err(connection_error(error));
        }
    }
    let stream = UnixStream::from(fd);
    stream
        .set_nonblocking(false)
        .map_err(|_| "owner_transport_unavailable")?;
    Ok(stream)
}
fn limits(stream: &UnixStream) -> Result<()> {
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .and_then(|_| stream.set_write_timeout(Some(Duration::from_secs(30))))
        .map_err(|_| "owner_transport_unavailable")
}
fn receive<T: DeserializeOwned>(stream: &mut UnixStream) -> Result<T> {
    let mut length = [0; 4];
    stream
        .read_exact(&mut length)
        .map_err(|_| "owner_response_unconfirmed")?;
    let size = u32::from_be_bytes(length) as usize;
    if size == 0 || size > MAX_MESSAGE {
        return Err("owner_message_bound");
    }
    let mut bytes = vec![0; size];
    stream
        .read_exact(&mut bytes)
        .map_err(|_| "owner_response_unconfirmed")?;
    serde_json::from_slice(&bytes).map_err(|_| "invalid_owner_message")
}
fn send(stream: &mut UnixStream, value: &impl Serialize) -> Result<()> {
    let bytes = serde_json::to_vec(value).map_err(|_| "invalid_owner_message")?;
    if bytes.is_empty() || bytes.len() > MAX_MESSAGE {
        return Err("owner_message_bound");
    }
    stream
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .and_then(|_| stream.write_all(&bytes))
        .map_err(|_| "owner_response_unconfirmed")
}
pub(crate) fn available(root: &JournalRoot) -> Result<bool> {
    let path = socket(root)?;
    if !path.try_exists().map_err(|_| "owner_socket_unavailable")? {
        return Ok(false);
    }
    validate_socket(&path)?;
    match call::<Value>(root, Action::Hello) {
        Ok(_) => Ok(true),
        Err("owner_connection_refused" | "owner_endpoint_absent") => {
            // A stale endpoint may survive SIGKILL. Fallback requires both a
            // failed live handshake and successful acquisition of the owner lock.
            let path = root.path.join("owner.lock");
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&path)
                .map_err(|_| "owner_unavailable; start ojo-journal serve")?;
            read_private(&path, 0).map_err(|_| "private_owner_lock_required")?;
            file.try_lock()
                .map_err(|_| "owner_unavailable; inspect the managed service")?;
            Ok(false)
        }
        Err(error) => Err(error),
    }
}
pub(crate) fn call<T: DeserializeOwned>(root: &JournalRoot, action: Action) -> Result<T> {
    let path = socket(root)?;
    validate_socket(&path)?;
    let mut stream = connect(&path)?;
    peer(&stream)?;
    limits(&stream)?;
    send(
        &mut stream,
        &Request {
            format: 1,
            root: root.identity().into(),
            action,
        },
    )?;
    let response: Response = receive(&mut stream)?;
    if response.format != 1 || response.root != root.identity() {
        return Err("owner_binding_mismatch");
    }
    if response.error.is_some() {
        return Err("owner_operation_refused; inspect the managed service log");
    }
    serde_json::from_value(response.value).map_err(|_| "invalid_owner_response")
}
fn execute(root: &mut JournalRoot, action: Action) -> Result<Value> {
    let value = match action {
        Action::Hello => root.owner_status()?.ok_or("owner_unhealthy")?,
        Action::Ensure {
            run,
            agent,
            adapter,
            label,
        } => json!(root.ensure_session(&run, &agent, &adapter, &label)?),
        Action::Append {
            run,
            request,
            records,
        } => json!(root.append(&run, &request, &records)?),
        Action::Manifest { run } => json!(root.manifest(&run)?),
        Action::Sessions => json!(root.sessions()?),
        Action::Get { run, sequence } => json!(root.get(&run, sequence)?),
        Action::Page { run, start, limit } => json!(root.page(&run, start, limit)?),
        Action::Search { run, query, limit } => json!(root.search(&run, &query, limit)?),
        Action::Finish { run, finished } => json!(root.finish(&run, finished)?),
        Action::Archive { run } => json!(root.archive(&run)?),
        Action::MigrateSession { run } => json!(root.migrate_session(&run)?),
        Action::ArchiveIdle { time_us } => json!(root.archive_idle(time_us)?),
        Action::ArchivePolicy { enabled } => {
            root.set_automatic_archive(enabled)?;
            Value::Null
        }
    };
    Ok(value)
}
struct SocketGuard(PathBuf);
impl Drop for SocketGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
fn listener(root: &JournalRoot) -> Result<(UnixListener, File, SocketGuard)> {
    let lock_path = root.path.join("owner.lock");
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&lock_path)
        .map_err(|_| "owner_lock_unavailable")?;
    read_private(&lock_path, 0).map_err(|_| "private_owner_lock_required")?;
    lock.try_lock().map_err(|_| "owner_already_running")?;
    let path = socket(root)?;
    let directory = path.parent().ok_or("invalid_owner_socket")?;
    if !directory.exists() {
        hosted_capture::directory(directory)?;
    }
    if path.exists() {
        validate_socket(&path)?;
        match connect(&path) {
            Err("owner_connection_refused" | "owner_endpoint_absent") => {}
            _ => return Err("owner_endpoint_already_live"),
        }
        fs::remove_file(&path).map_err(|_| "stale_owner_socket_unavailable")?;
    }
    // Validate the private parent before bind, even for the first socket.
    let m = fs::symlink_metadata(directory).map_err(|_| "owner_directory_unavailable")?;
    if !m.is_dir() || m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o777 != 0o700 {
        return Err("private_owner_directory_required");
    }
    let listener = UnixListener::bind(&path).map_err(|_| "owner_bind_failed")?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
        .map_err(|_| "private_owner_socket_required")?;
    Ok((listener, lock, SocketGuard(path)))
}
pub(crate) fn serve(path: &std::path::Path, cloud: bool) -> Result<()> {
    let root = JournalRoot::open_standalone(path)?.owned();
    let (listener, _lock, _socket) = listener(&root)?;
    let count = Arc::new(AtomicUsize::new(0));
    for adapter in ["codex", "claude-code"] {
        let root = root.clone();
        std::thread::spawn(move || {
            loop {
                if root.path.join(format!("capture-{adapter}.json")).exists()
                    && let Err(error) = super::capture_once(&root, adapter)
                {
                    eprintln!("ojo-journal: capture_pending: {error}");
                }
                std::thread::sleep(Duration::from_secs(2));
            }
        });
    }
    if cloud {
        let root = root.clone();
        std::thread::spawn(move || {
            loop {
                if root.path.join("cloud.json").exists()
                    && let Err(error) = super::sync_cloud(&root, &root.path.join("cloud.json"), 8)
                {
                    eprintln!("ojo-journal: sync_pending: {error}");
                }
                std::thread::sleep(Duration::from_secs(5));
            }
        });
    }
    {
        let root = root.clone();
        std::thread::spawn(move || {
            loop {
                if let Err(error) = root.archive_idle(super::store::now()) {
                    eprintln!("ojo-journal: archive_pending: {error}");
                }
                std::thread::sleep(Duration::from_secs(3600));
            }
        });
    }
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else {
            return Err("owner_listener_failed");
        };
        if peer(&stream).is_err() {
            continue;
        }
        if count.fetch_add(1, Ordering::AcqRel) >= 4 {
            count.fetch_sub(1, Ordering::AcqRel);
            continue;
        }
        let count = count.clone();
        let mut root = root.clone();
        std::thread::spawn(move || {
            let result = (|| -> Result<()> {
                limits(&stream)?;
                let request: Request = receive(&mut stream)?;
                if request.format != 1 || request.root != root.identity() {
                    return Err("owner_binding_mismatch");
                }
                let result = execute(&mut root, request.action);
                let (value, error) = match result {
                    Ok(value) => (value, None),
                    Err(error) => {
                        eprintln!("ojo-journal: owner_request_refused: {error}");
                        (Value::Null, Some(error.into()))
                    }
                };
                send(
                    &mut stream,
                    &Response {
                        format: 1,
                        root: root.identity().into(),
                        value,
                        error,
                    },
                )
            })();
            if let Err(error) = result {
                eprintln!("ojo-journal: owner_transport: {error}");
            }
            count.fetch_sub(1, Ordering::AcqRel);
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (PathBuf, JournalRoot, UnixListener, SocketGuard) {
        let scratch = std::env::temp_dir().join(format!(
            "ojo-owner-check-{}",
            hosted_capture::random_id().unwrap()
        ));
        hosted_capture::directory(&scratch).unwrap();
        let root = JournalRoot::init(&scratch.join("root"), false).unwrap();
        hosted_capture::create_private(&root.path.join("owner.lock"), b"").unwrap();
        let path = socket(&root).unwrap();
        let parent = path.parent().unwrap();
        if !parent.exists() {
            hosted_capture::directory(parent).unwrap();
        }
        let listener = UnixListener::bind(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        (scratch, root, listener, SocketGuard(path))
    }
    #[test]
    fn a_responsive_conflicting_owner_never_enables_direct_fallback() {
        let (scratch, root, listener, guard) = fixture();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            limits(&stream).unwrap();
            let _: Request = receive(&mut stream).unwrap();
            send(
                &mut stream,
                &Response {
                    format: 1,
                    root: "f".repeat(64),
                    value: json!({}),
                    error: None,
                },
            )
            .unwrap();
        });
        assert!(matches!(
            JournalRoot::open(&root.path),
            Err("owner_binding_mismatch")
        ));
        server.join().unwrap();
        drop(guard);
        fs::remove_dir_all(scratch).unwrap();
    }
    #[test]
    fn full_backlog_is_live_and_never_unlinked_as_a_stale_socket() {
        let (scratch, root, bound, guard) = fixture();
        assert_eq!(unsafe { libc::listen(bound.as_raw_fd(), 1) }, 0);
        let path = socket(&root).unwrap();
        let before = fs::symlink_metadata(&path).unwrap().ino();
        let mut held = Vec::new();
        let mut busy = false;
        for _ in 0..8 {
            match connect(&path) {
                Ok(stream) => held.push(stream),
                Err("owner_backlog_busy") => {
                    busy = true;
                    break;
                }
                Err(error) => panic!("{error}"),
            }
        }
        assert!(busy);
        assert!(matches!(
            listener(&root),
            Err("owner_endpoint_already_live")
        ));
        assert_eq!(fs::symlink_metadata(&path).unwrap().ino(), before);
        drop(held);
        drop(bound);
        drop(guard);
        fs::remove_dir_all(scratch).unwrap();
    }
}
