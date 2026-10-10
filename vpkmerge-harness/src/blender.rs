//! Snapshot adapter for the Blender MCP *addon's* local TCP protocol.
//!
//! This is the addon's own newline-free JSON-over-TCP socket (`{"type", "params"}`
//! in, `{"status", "result" | "message"}` out). It is not MCP transport and does
//! no MCP handshake. Only two read-oriented commands are ever sent, to loopback
//! IPv4; `execute_code` is deliberately unreachable from this module.
//!
//! The screenshot command makes Blender write a file. We hand it a path inside an
//! app-owned temporary directory and read only that path back, never a path the
//! peer reports.

use anyhow::{anyhow, bail, ensure, Context, Result};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::sync::{Mutex, MutexGuard, TryLockError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Requested (and enforced) maximum screenshot dimension.
pub const SNAPSHOT_MAX_SIZE: u32 = 1280;

/// Upper bound on decoder allocations while validating a screenshot.
const PNG_MAX_ALLOC: u64 = 32 * 1024 * 1024;

/// One backend-wide lock so scene queries and captures never overlap. It is
/// never waited on: a second caller fails fast with a "busy" error instead of
/// queueing outside its own deadline.
static BACKEND: Mutex<()> = Mutex::new(());

fn acquire_backend() -> Result<MutexGuard<'static, ()>> {
    match BACKEND.try_lock() {
        Ok(guard) => Ok(guard),
        Err(TryLockError::Poisoned(e)) => Ok(e.into_inner()),
        Err(TryLockError::WouldBlock) => {
            bail!("another Blender request is already in progress; try again shortly")
        }
    }
}

#[derive(Debug, Clone)]
pub struct Limits {
    pub connect_timeout: Duration,
    pub read_timeout: Duration,
    pub write_timeout: Duration,
    /// Wall-clock budget for the whole operation, including file validation.
    pub deadline: Duration,
    pub max_response_bytes: usize,
    pub max_png_bytes: u64,
}

impl Limits {
    #[must_use]
    pub fn scene() -> Self {
        Self {
            connect_timeout: Duration::from_secs(2),
            read_timeout: Duration::from_secs(10),
            write_timeout: Duration::from_secs(5),
            deadline: Duration::from_secs(15),
            max_response_bytes: 1024 * 1024,
            max_png_bytes: 0,
        }
    }

    #[must_use]
    pub fn snapshot() -> Self {
        Self {
            deadline: Duration::from_secs(20),
            read_timeout: Duration::from_secs(15),
            max_response_bytes: 16 * 1024,
            max_png_bytes: 8 * 1024 * 1024,
            ..Self::scene()
        }
    }
}

#[derive(Debug)]
pub struct Snapshot {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// Unix time in milliseconds when this process finished receiving and
    /// validating the image. This is receipt time, not the moment the GPU
    /// rendered the frame.
    pub captured_at_ms: u64,
}

/// Return the addon's `get_scene_info` result, unwrapped from its envelope.
pub fn scene_info(port: u16) -> Result<Value> {
    scene_info_with(port, &Limits::scene())
}

pub fn scene_info_with(port: u16, limits: &Limits) -> Result<Value> {
    let _guard = acquire_backend()?;
    fetch_scene(port, limits)
}

/// Lock-free core of [`scene_info_with`].
fn fetch_scene(port: u16, limits: &Limits) -> Result<Value> {
    let deadline = Instant::now() + limits.deadline;
    let scene = request(
        port,
        &json!({"type": "get_scene_info", "params": {}}),
        limits,
        deadline,
    )?;
    ensure!(scene.is_object(), "Blender scene response is malformed");
    Ok(scene)
}

/// Ask Blender to save its viewport to a private temp file and return the PNG.
pub fn viewport_snapshot(port: u16) -> Result<Snapshot> {
    viewport_snapshot_with(port, &Limits::snapshot())
}

pub fn viewport_snapshot_with(port: u16, limits: &Limits) -> Result<Snapshot> {
    let _guard = acquire_backend()?;
    fetch_snapshot(port, limits)
}

/// Lock-free core of [`viewport_snapshot_with`].
fn fetch_snapshot(port: u16, limits: &Limits) -> Result<Snapshot> {
    let deadline = Instant::now() + limits.deadline;
    // Removed on drop, on every exit path.
    let dir = tempfile::Builder::new()
        .prefix("vpkmerge-blender-")
        .tempdir()
        .context("creating private snapshot directory")?;
    let path = dir.path().join("viewport.png");
    let path_str = path
        .to_str()
        .ok_or_else(|| anyhow!("snapshot path is not valid UTF-8"))?;

    let result = request(
        port,
        &json!({"type": "get_viewport_screenshot", "params": {
            "max_size": SNAPSHOT_MAX_SIZE, "filepath": path_str, "format": "png"
        }}),
        limits,
        deadline,
    )?;
    if result.get("success").and_then(Value::as_bool) == Some(false) {
        bail!("Blender did not report a successful screenshot");
    }
    ensure!(
        Instant::now() < deadline,
        "timed out waiting for the Blender screenshot"
    );

    let png = read_private_file(&path, limits.max_png_bytes)?;
    let (width, height) = validate_png(&png)?;
    ensure!(
        Instant::now() < deadline,
        "timed out validating the Blender screenshot"
    );
    let captured_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
    Ok(Snapshot {
        png,
        width,
        height,
        captured_at_ms,
    })
}

fn read_private_file(path: &std::path::Path, max: u64) -> Result<Vec<u8>> {
    let meta = std::fs::symlink_metadata(path)
        .map_err(|_| anyhow!("Blender reported success but did not create the screenshot file"))?;
    ensure!(meta.is_file(), "screenshot output is not a regular file");
    ensure!(meta.len() > 0, "screenshot file is empty");
    ensure!(
        meta.len() <= max,
        "screenshot file is too large ({} bytes, limit {max})",
        meta.len()
    );
    let capacity = usize::try_from(meta.len()).context("screenshot file size overflows usize")?;
    let mut buf = Vec::with_capacity(capacity);
    std::fs::File::open(path)
        .context("opening screenshot file")?
        .take(max.saturating_add(1))
        .read_to_end(&mut buf)
        .context("reading screenshot file")?;
    let read = u64::try_from(buf.len()).unwrap_or(u64::MAX);
    ensure!(read <= max, "screenshot file is too large");
    Ok(buf)
}

/// Fully decode the PNG (header, CRCs, pixel data) under explicit dimension
/// and allocation limits. The header dimensions are checked before any pixel
/// buffer is allocated.
pub fn validate_png(bytes: &[u8]) -> Result<(u32, u32)> {
    use image::{DynamicImage, ImageDecoder};

    const SIG: &[u8] = b"\x89PNG\r\n\x1a\n";
    // Length 0, type "IEND", CRC of an empty IEND body.
    const IEND: &[u8] = b"\x00\x00\x00\x00IEND\xaeB`\x82";
    ensure!(bytes.starts_with(SIG), "screenshot is not a PNG");
    ensure!(
        bytes.len() >= SIG.len() + IEND.len() && bytes.ends_with(IEND),
        "screenshot PNG is truncated (missing terminal IEND chunk)"
    );
    let mut limits = image::Limits::no_limits();
    limits.max_image_width = Some(SNAPSHOT_MAX_SIZE);
    limits.max_image_height = Some(SNAPSHOT_MAX_SIZE);
    limits.max_alloc = Some(PNG_MAX_ALLOC);
    let decoder = image::codecs::png::PngDecoder::with_limits(std::io::Cursor::new(bytes), limits)
        .context("screenshot PNG has no valid header or exceeds decoder limits")?;
    let (width, height) = decoder.dimensions();
    ensure!(
        (1..=SNAPSHOT_MAX_SIZE).contains(&width) && (1..=SNAPSHOT_MAX_SIZE).contains(&height),
        "screenshot dimensions {width}x{height} are outside 1..={SNAPSHOT_MAX_SIZE}"
    );
    DynamicImage::from_decoder(decoder).context("screenshot PNG is corrupt or truncated")?;
    Ok((width, height))
}

fn request(port: u16, command: &Value, limits: &Limits, deadline: Instant) -> Result<Value> {
    ensure!(port > 0, "port must be between 1 and 65535");
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let mut stream =
        TcpStream::connect_timeout(&addr, remaining(deadline)?.min(limits.connect_timeout))
            .with_context(|| {
                format!("connecting to Blender addon at {addr} (is its server running?)")
            })?;
    stream.set_write_timeout(Some(remaining(deadline)?.min(limits.write_timeout)))?;
    stream
        .write_all(&serde_json::to_vec(command)?)
        .context("sending command to Blender")?;
    unwrap_envelope(&read_json(&mut stream, limits, deadline)?)
}

fn remaining(deadline: Instant) -> Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|d| !d.is_zero())
        .ok_or_else(|| anyhow!("timed out waiting for Blender"))
}

/// Accumulate reads until one complete JSON value parses. Blender keeps the
/// connection open after replying, so we must not wait for EOF.
fn read_json(stream: &mut TcpStream, limits: &Limits, deadline: Instant) -> Result<Value> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        stream.set_read_timeout(Some(remaining(deadline)?.min(limits.read_timeout)))?;
        match stream.read(&mut chunk) {
            Ok(0) => bail!("Blender closed the connection before sending a complete response"),
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                ensure!(
                    buf.len() <= limits.max_response_bytes,
                    "Blender response exceeded {} bytes",
                    limits.max_response_bytes
                );
                match serde_json::from_slice::<Value>(&buf) {
                    Ok(value) => return Ok(value),
                    Err(e) if e.is_eof() => {}
                    Err(e) => bail!("Blender sent an invalid response: {e}"),
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                bail!("timed out waiting for Blender")
            }
            Err(e) => return Err(e).context("reading Blender response"),
        }
    }
}

fn describe(value: &Value) -> String {
    let text = value
        .as_str()
        .map_or_else(|| value.to_string(), str::to_owned);
    text.chars().take(300).collect()
}

/// Handle `{"status":"error","message"}` and `{"status":"success","result":{"error"}}`.
fn unwrap_envelope(value: &Value) -> Result<Value> {
    match value.get("status").and_then(Value::as_str) {
        Some("success") => {
            let result = value
                .get("result")
                .cloned()
                .ok_or_else(|| anyhow!("Blender response is missing `result`"))?;
            ensure!(
                result.is_object(),
                "Blender response `result` is not an object"
            );
            if let Some(err) = result.get("error") {
                bail!("Blender reported an error: {}", describe(err));
            }
            Ok(result)
        }
        Some("error") => bail!(
            "Blender reported an error: {}",
            value
                .get("message")
                .map_or_else(|| "no message".into(), describe)
        ),
        _ => bail!("unexpected response from Blender addon"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread::JoinHandle;

    fn test_limits() -> Limits {
        Limits {
            connect_timeout: Duration::from_secs(2),
            read_timeout: Duration::from_millis(300),
            write_timeout: Duration::from_secs(2),
            deadline: Duration::from_secs(5),
            max_response_bytes: 4096,
            max_png_bytes: 64 * 1024,
        }
    }

    // Shadow the public, globally locked entry points with the lock-free cores
    // so parallel tests never contend on the shared backend lock. The lock
    // itself is covered by `concurrent_request_is_rejected_as_busy`.
    fn scene_info_with(port: u16, limits: &Limits) -> Result<Value> {
        fetch_scene(port, limits)
    }

    fn viewport_snapshot_with(port: u16, limits: &Limits) -> Result<Snapshot> {
        fetch_snapshot(port, limits)
    }

    /// A real, fully valid PNG produced by the image encoder.
    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut out = Vec::new();
        image::RgbaImage::new(width, height)
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    /// Serve one connection. `handler` gets the parsed request and the stream;
    /// afterwards the server waits for the client to hang up, mimicking the
    /// addon, which keeps the socket open after replying. Every wait is bounded
    /// so a failing test cannot hang the suite.
    fn serve(
        handler: impl FnOnce(Value, &mut TcpStream) + Send + 'static,
    ) -> (u16, JoinHandle<()>) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            let give_up = Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < give_up, "no client connected");
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(e) => panic!("accept failed: {e}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut buf = Vec::new();
            let mut chunk = [0u8; 1024];
            let request = loop {
                let n = stream.read(&mut chunk).unwrap();
                assert!(n > 0, "client closed before sending a request");
                buf.extend_from_slice(&chunk[..n]);
                if let Ok(v) = serde_json::from_slice::<Value>(&buf) {
                    break v;
                }
            };
            handler(request, &mut stream);
            let mut sink = [0u8; 64];
            while matches!(stream.read(&mut sink), Ok(n) if n > 0) {}
        });
        (port, handle)
    }

    #[test]
    fn scene_reads_fragmented_reply_without_eof() {
        let (port, server) = serve(|req, stream| {
            assert_eq!(req, json!({"type": "get_scene_info", "params": {}}));
            let reply = br#"{"status":"success","result":{"name":"Scene","object_count":3}}"#;
            for part in reply.chunks(7) {
                stream.write_all(part).unwrap();
                stream.flush().unwrap();
                std::thread::sleep(Duration::from_millis(2));
            }
        });
        let scene = scene_info_with(port, &test_limits()).unwrap();
        assert_eq!(scene, json!({"name": "Scene", "object_count": 3}));
        server.join().unwrap();
    }

    #[test]
    fn scene_reports_remote_and_nested_errors() {
        let (port, server) = serve(|_, s| {
            s.write_all(br#"{"status":"error","message":"boom"}"#)
                .unwrap();
        });
        let err = scene_info_with(port, &test_limits())
            .unwrap_err()
            .to_string();
        assert!(err.contains("boom"), "{err}");
        server.join().unwrap();

        let (port, server) = serve(|_, s| {
            s.write_all(br#"{"status":"success","result":{"error":"No 3D viewport found"}}"#)
                .unwrap();
        });
        let err = scene_info_with(port, &test_limits())
            .unwrap_err()
            .to_string();
        assert!(err.contains("No 3D viewport found"), "{err}");
        server.join().unwrap();
    }

    #[test]
    fn oversized_reply_is_rejected() {
        let (port, server) = serve(|_, s| {
            let mut junk = br#"{"status":"success","result":""#.to_vec();
            junk.extend(std::iter::repeat_n(b'a', 16 * 1024));
            let _ = s.write_all(&junk); // client may reset mid-write
        });
        let err = scene_info_with(port, &test_limits())
            .unwrap_err()
            .to_string();
        assert!(err.contains("exceeded"), "{err}");
        server.join().unwrap();
    }

    #[test]
    fn silent_peer_times_out() {
        let (port, server) = serve(|_, _| {});
        let mut limits = test_limits();
        limits.read_timeout = Duration::from_millis(100);
        let err = scene_info_with(port, &limits).unwrap_err().to_string();
        assert!(err.contains("timed out"), "{err}");
        server.join().unwrap();
    }

    #[test]
    fn early_close_and_refused_connection_fail() {
        let (port, server) = serve(|_, s| {
            s.write_all(br#"{"status":"succ"#).unwrap();
            s.shutdown(std::net::Shutdown::Both).unwrap();
        });
        let err = scene_info_with(port, &test_limits())
            .unwrap_err()
            .to_string();
        assert!(err.contains("closed"), "{err}");
        server.join().unwrap();

        let port = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        assert!(scene_info_with(port, &test_limits()).is_err());
        assert!(scene_info_with(0, &test_limits()).is_err());
    }

    type SnapshotServer = (
        u16,
        JoinHandle<()>,
        std::sync::mpsc::Receiver<std::path::PathBuf>,
    );

    fn snapshot_server(write: impl FnOnce(&std::path::Path) + Send + 'static) -> SnapshotServer {
        let (tx, rx) = std::sync::mpsc::channel();
        let (port, handle) = serve(move |req, stream| {
            assert_eq!(req["type"], "get_viewport_screenshot");
            assert_eq!(req["params"]["max_size"], 1280);
            assert_eq!(req["params"]["format"], "png");
            let path = std::path::PathBuf::from(req["params"]["filepath"].as_str().unwrap());
            write(&path);
            tx.send(path).unwrap();
            // The reply's own `filepath` must be ignored.
            stream
                .write_all(
                    br#"{"status":"success","result":{"success":true,"width":2,"height":3,"filepath":"/etc/passwd"}}"#,
                )
                .unwrap();
        });
        (port, handle, rx)
    }

    #[test]
    fn snapshot_returns_png_and_cleans_up() {
        let (port, server, rx) = snapshot_server(|p| std::fs::write(p, png(2, 3)).unwrap());
        let snap = viewport_snapshot_with(port, &test_limits()).unwrap();
        assert_eq!((snap.width, snap.height), (2, 3));
        assert_eq!(snap.png, png(2, 3));
        assert!(snap.captured_at_ms > 0);
        let path = rx.recv().unwrap();
        assert!(!path.parent().unwrap().exists(), "tempdir must be removed");
        server.join().unwrap();
    }

    #[test]
    fn snapshot_rejects_missing_invalid_and_oversized_files() {
        let (port, server, rx) = snapshot_server(|_| {});
        let err = viewport_snapshot_with(port, &test_limits())
            .unwrap_err()
            .to_string();
        assert!(err.contains("did not create"), "{err}");
        assert!(!rx.recv().unwrap().parent().unwrap().exists());
        server.join().unwrap();

        let (port, server, _rx) = snapshot_server(|p| {
            std::fs::write(p, b"not a png at all, sorry, really not one").unwrap();
        });
        let err = viewport_snapshot_with(port, &test_limits())
            .unwrap_err()
            .to_string();
        assert!(err.contains("not a PNG"), "{err}");
        server.join().unwrap();

        let (port, server, _rx) = snapshot_server(|p| std::fs::write(p, png(4000, 10)).unwrap());
        let err = viewport_snapshot_with(port, &test_limits())
            .unwrap_err()
            .to_string();
        assert!(err.contains("decoder limits"), "{err}");
        server.join().unwrap();

        let (port, server, _rx) = snapshot_server(|p| std::fs::write(p, png(2, 2)).unwrap());
        let mut limits = test_limits();
        limits.max_png_bytes = 10;
        let err = viewport_snapshot_with(port, &limits)
            .unwrap_err()
            .to_string();
        assert!(err.contains("too large"), "{err}");
        server.join().unwrap();
    }

    #[test]
    fn png_validation_detects_truncation_and_corruption() {
        let mut bytes = png(5, 5);
        assert_eq!(validate_png(&bytes).unwrap(), (5, 5));

        let mut corrupt = bytes.clone();
        let idx = corrupt.len() - 12 - 5; // inside the final IDAT chunk
        corrupt[idx] ^= 0xff;
        assert!(validate_png(&corrupt).is_err());

        bytes.truncate(bytes.len() - 1);
        assert!(validate_png(&bytes).is_err());
    }

    #[test]
    fn png_validation_rejects_fake_header_only_files() {
        // Signature, IHDR with a bogus CRC, and IEND, but no pixel data.
        let mut fake = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR".to_vec();
        fake.extend(5u32.to_be_bytes());
        fake.extend(5u32.to_be_bytes());
        fake.extend([8, 6, 0, 0, 0, 0, 0, 0, 0]);
        fake.extend(b"\x00\x00\x00\x00IEND\xaeB`\x82");
        assert!(validate_png(&fake).is_err());

        // A valid header and IEND with the image data stripped out.
        let valid = png(5, 5);
        let idat = valid.windows(4).position(|w| w == b"IDAT").unwrap() - 4;
        let mut stripped = valid[..idat].to_vec();
        stripped.extend(&valid[valid.len() - 12..]);
        assert!(validate_png(&stripped).is_err());
    }

    #[test]
    fn png_validation_enforces_dimension_bounds() {
        let max = SNAPSHOT_MAX_SIZE;
        assert_eq!(validate_png(&png(max, 1)).unwrap(), (max, 1));
        assert!(validate_png(&png(max + 1, 1)).is_err());
        assert!(validate_png(&png(1, max + 1)).is_err());
    }

    #[test]
    fn scene_must_be_an_object() {
        for body in [
            &br#"{"status":"success","result":null}"#[..],
            br#"{"status":"success","result":42}"#,
            br#"{"status":"success","result":"ok"}"#,
        ] {
            let (port, server) = serve(move |_, s| s.write_all(body).unwrap());
            let err = scene_info_with(port, &test_limits())
                .unwrap_err()
                .to_string();
            assert!(err.contains("not an object"), "{err}");
            server.join().unwrap();
        }
    }

    #[test]
    fn concurrent_request_is_rejected_as_busy() {
        let _held = acquire_backend().unwrap();
        let err = super::scene_info_with(1, &test_limits())
            .unwrap_err()
            .to_string();
        assert!(err.contains("already in progress"), "{err}");
        let err = super::viewport_snapshot_with(1, &test_limits())
            .unwrap_err()
            .to_string();
        assert!(err.contains("already in progress"), "{err}");
    }
}
