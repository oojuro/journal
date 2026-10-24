//! Synthetic fixture builder for actual Temporal CLI/SDK process benchmarks.
//! Requires an empty native setup with the exact benchmark identity; no discovery/network.
use oojuro_capture::{
    Rec, ingest::{Decision, Policy}, kind,
    local::{LocalCapture, Options, Stream},
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{fs, io::Read, os::unix::fs::{OpenOptionsExt, PermissionsExt}, path::Path, time::{Duration, Instant}};

struct Allow;
impl Policy for Allow {
    fn decide(&mut self, rec: &Rec, _: &str) -> Decision {
        Decision::Store { indexed: kind::indexed(rec.kind) }
    }
}
fn private(path: &Path) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut file = fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW).open(path)?;
    if !file.metadata()?.is_file() || file.metadata()?.permissions().mode() & 0o777 != 0o600 {
        return Err("private synthetic fixture required".into());
    }
    let mut bytes = Vec::new();
    file.by_ref().take(65537).read_to_end(&mut bytes)?;
    if bytes.len() > 65536 { return Err("fixture configuration bound".into()); }
    Ok(bytes)
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() != 3 { return Err("seed_temporal_cli SYNTHETIC_ROOT COUNT".into()); }
    let root = Path::new(&args[1]);
    let count: usize = args[2].to_str().ok_or("count")?.parse()?;
    if !(20..=10000).contains(&count) || !fs::symlink_metadata(root)?.is_dir()
        || fs::metadata(root)?.permissions().mode() & 0o777 != 0o700 {
        return Err("private synthetic root and 20..10000 events required".into());
    }
    let config: serde_json::Value = serde_json::from_slice(&private(&root.join("native.json"))?)?;
    if config != json!({"version":1,"harness":"temporal","writer":"temporal:synthetic-benchmark","agent":7,"first_id":1}) {
        return Err("synthetic benchmark identity required".into());
    }
    let policy = private(&root.join("policy.json"))?;
    if serde_json::from_slice::<serde_json::Value>(&policy)? != json!({"version":1,"record":true,"paused":false,
        "record_thinking":true,"index_thinking":true,"exclude":[],"denied_tools":[]}) {
        return Err("synthetic benchmark policy required".into());
    }
    let key: [u8;32] = private(&root.join("key"))?.try_into().map_err(|_| "key size")?;
    let mut options = Options::new("temporal:synthetic-benchmark", 7,
        &format!("native-policy-1:{}", blake3::hash(&policy).to_hex()));
    options.raw_authorization = "native-cli-explicit-source-1".into();
    let mut capture = LocalCapture::open_temporal(&root.join("journal"), &key, options, Box::new(Allow))?;
    if capture.authority()?.sources().next().is_some() || capture.authority()?.next_id() != 1 {
        return Err("empty synthetic authority required".into());
    }
    let stream = Stream { namespace:"synthetic".into(), workflow_id:"workflow".into(),
        run_id:"run".into(), generation:"generation-1".into() };
    let source = stream.source_id()?;
    let began = Instant::now();
    let mut cursor = 0;
    let mut digest = Sha256::new();
    for i in 0..count {
        if began.elapsed() > Duration::from_secs(120) { return Err("synthetic seed time bound".into()); }
        let mut frame = serde_json::to_vec(&json!({"version":1,"id":format!("seed-{i}"),"sequence":i+1,
            "valid_us":123456,"kind":"input","tool":null,"operation_id":null,"status":null,
            "payload":format!("synthetic Temporal needle{i:08} λ\r\n\0終"),"attributes":{}}))?;
        frame.push(b'\n');
        let ack = capture.append_temporal(source, &stream, cursor, &frame, true)?;
        if ack.outcome.first_id != Some(i as u64 + 1) { return Err("synthetic ID mismatch".into()); }
        cursor = ack.end;
        digest.update(&frame);
    }
    let mut restored = Vec::new();
    capture.restore_raw(source, &mut restored, true)?;
    let expected = format!("{:x}", digest.finalize());
    if format!("{:x}", Sha256::digest(&restored)) != expected { return Err("seed raw mismatch".into()); }
    println!("{}",json!({"records":count,"bytes":cursor,"raw_sha256":expected,
        "elapsed_ms":began.elapsed().as_millis(),"source_id":source.iter().map(|b|format!("{b:02x}")).collect::<String>()}));
    Ok(())
}
