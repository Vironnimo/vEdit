//! Private, bounded control transport for one running vEdit window.
//!
//! Only the GUI thread applies requests. A network acknowledgement is therefore
//! an application result, never merely an acknowledgement of socket delivery.

use super::{Request, Response};
use eframe::egui;
use serde::de::{DeserializeOwned, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const PROTOCOL_VERSION: u32 = 1;
const MAX_REQUEST_BYTES: usize = 1024 * 1024;
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const MAX_CONNECTIONS: usize = 8;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Endpoint {
    version: u32,
    pid: u32,
    port: u16,
    token: String,
}

#[derive(Serialize)]
struct Outgoing<'a> {
    version: u32,
    token: &'a str,
    request: &'a Request,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Incoming {
    version: u32,
    token: String,
    request: serde_json::Value,
}

/// Decode strict JSON without silently choosing the last of contradictory keys.
/// This also serves the CLI's raw requests and review manifest reader.
pub(super) fn from_unique_json<T: DeserializeOwned>(bytes: &[u8]) -> serde_json::Result<T> {
    let value: UniqueValue = serde_json::from_slice(bytes)?;
    serde_json::from_value(value.0)
}

struct UniqueValue(serde_json::Value);

impl<'de> Deserialize<'de> for UniqueValue {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(UniqueValueVisitor)
    }
}

struct UniqueValueVisitor;

impl<'de> Visitor<'de> for UniqueValueVisitor {
    type Value = UniqueValue;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a JSON value with unique object keys")
    }

    fn visit_bool<E: serde::de::Error>(self, value: bool) -> Result<Self::Value, E> {
        Ok(UniqueValue(value.into()))
    }

    fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<Self::Value, E> {
        Ok(UniqueValue(value.into()))
    }

    fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<Self::Value, E> {
        Ok(UniqueValue(value.into()))
    }

    fn visit_f64<E: serde::de::Error>(self, value: f64) -> Result<Self::Value, E> {
        serde_json::Number::from_f64(value)
            .map(|number| UniqueValue(number.into()))
            .ok_or_else(|| E::custom("JSON numbers must be finite"))
    }

    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
        self.visit_string(value.to_owned())
    }

    fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Self::Value, E> {
        Ok(UniqueValue(value.into()))
    }

    fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
        Ok(UniqueValue(serde_json::Value::Null))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Self::Value, A::Error> {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element::<UniqueValue>()? {
            values.push(value.0);
        }
        Ok(UniqueValue(values.into()))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut object: A) -> Result<Self::Value, A::Error> {
        let mut values = serde_json::Map::new();
        while let Some(key) = object.next_key::<String>()? {
            if values.contains_key(&key) {
                return Err(serde::de::Error::custom(format!(
                    "Duplicate JSON object key: {key}"
                )));
            }
            values.insert(key, object.next_value::<UniqueValue>()?.0);
        }
        Ok(UniqueValue(values.into()))
    }
}

pub struct Envelope {
    pub request: Request,
    pub reply: mpsc::Sender<Response>,
}

pub struct Server {
    receiver: mpsc::Receiver<Envelope>,
    stop: Arc<AtomicBool>,
    listener: Option<JoinHandle<()>>,
    address: SocketAddr,
    endpoint_path: PathBuf,
    token: String,
    #[cfg(test)]
    wakeups: Arc<AtomicUsize>,
}

impl Server {
    #[cfg(test)]
    pub fn start_test(ctx: egui::Context, directory: PathBuf) -> Result<Self, String> {
        Self::start_in(ctx, directory)
    }
    pub fn start(ctx: egui::Context) -> Result<Self, String> {
        Self::start_in(ctx, endpoint_directory())
    }

    fn start_in(ctx: egui::Context, directory: PathBuf) -> Result<Self, String> {
        private_directory(&directory)
            .map_err(|error| format!("Could not create the vEdit control directory: {error}"))?;
        let endpoint_path = directory.join("control.json");
        if let Ok(endpoint) = read_endpoint(&endpoint_path)
            && TcpStream::connect_timeout(&endpoint_address(&endpoint), CONNECT_TIMEOUT).is_ok()
        {
            return Err("Another vEdit control server is already running.".into());
        }
        // The listener blocks in accept() instead of polling, so an idle editor
        // has no periodic wakeups here. Dropping the server wakes it once.
        let socket = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .map_err(|error| format!("Could not bind the vEdit control server: {error}"))?;
        let token = random_token()
            .map_err(|error| format!("Could not create a private control token: {error}"))?;
        let port = socket
            .local_addr()
            .map_err(|error| format!("Could not inspect the control server address: {error}"))?
            .port();
        let endpoint = Endpoint {
            version: PROTOCOL_VERSION,
            pid: std::process::id(),
            port,
            token: token.clone(),
        };
        write_endpoint(&endpoint_path, &endpoint)
            .map_err(|error| format!("Could not publish the vEdit control endpoint: {error}"))?;
        let (sender, receiver) = mpsc::sync_channel(32);
        let stop = Arc::new(AtomicBool::new(false));
        let listener_stop = stop.clone();
        let active = Arc::new(AtomicUsize::new(0));
        #[cfg(test)]
        let wakeups = Arc::new(AtomicUsize::new(0));
        #[cfg(test)]
        let listener_wakeups = wakeups.clone();
        let listener = thread::Builder::new()
            .name("vedit-control".into())
            .spawn(move || {
                loop {
                    let accepted = socket.accept();
                    #[cfg(test)]
                    listener_wakeups.fetch_add(1, Ordering::Relaxed);
                    if listener_stop.load(Ordering::Acquire) {
                        break;
                    }
                    match accepted {
                        Ok((stream, address)) => {
                            if !address.ip().is_loopback()
                                || active.load(Ordering::Acquire) >= MAX_CONNECTIONS
                            {
                                // Do not spend the GUI's time on unauthenticated overload.
                                drop(stream);
                                continue;
                            }
                            active.fetch_add(1, Ordering::AcqRel);
                            let worker_active = active.clone();
                            let worker_stop = listener_stop.clone();
                            let worker_sender = sender.clone();
                            let worker_token = endpoint.token.clone();
                            let worker_ctx = ctx.clone();
                            let result = thread::Builder::new()
                                .name("vedit-control-request".into())
                                .spawn(move || {
                                    let _connection = ConnectionCount(worker_active);
                                    serve_connection(
                                        stream,
                                        &worker_token,
                                        &worker_sender,
                                        &worker_stop,
                                        &worker_ctx,
                                    );
                                });
                            if result.is_err() {
                                active.fetch_sub(1, Ordering::AcqRel);
                            }
                        }
                        Err(_) => break,
                    }
                }
            })
            .map_err(|error| {
                let _ = fs::remove_file(&endpoint_path);
                format!("Could not start the vEdit control server: {error}")
            })?;
        Ok(Self {
            receiver,
            stop,
            listener: Some(listener),
            address: SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
            endpoint_path,
            token,
            #[cfg(test)]
            wakeups,
        })
    }

    pub fn try_recv(&self) -> Option<Envelope> {
        self.receiver.try_recv().ok()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(listener) = self.listener.take() {
            // One loopback connection wakes the blocking accept so the listener
            // observes the stop flag and releases its port. Joining without
            // that wakeup could block shutdown, so it is only skipped when the
            // listener cannot be reached and has not already finished.
            let woken = TcpStream::connect_timeout(&self.address, CONNECT_TIMEOUT).is_ok();
            if woken || listener.is_finished() {
                let _ = listener.join();
            }
        }
        // A replacement server must never lose its endpoint when an older
        // process finishes shutting down.
        if read_endpoint(&self.endpoint_path).is_ok_and(|endpoint| endpoint.token == self.token) {
            let _ = fs::remove_file(&self.endpoint_path);
        }
    }
}

struct ConnectionCount(Arc<AtomicUsize>);

impl Drop for ConnectionCount {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

pub fn endpoint_directory() -> PathBuf {
    std::env::var_os("VEDIT_CONTROL_DIR")
        .or_else(|| std::env::var_os("MORROW_CONTROL_DIR"))
        .map(PathBuf::from)
        .unwrap_or_else(crate::profile::recovery_directory)
}

pub fn send(request: &Request) -> Result<Response, String> {
    send_to(request, &endpoint_directory().join("control.json"))
}

/// A startup preflight only. Never use a failed probe to replay a request whose
/// bytes might already have reached the previous server.
pub fn endpoint_alive() -> bool {
    read_endpoint(&endpoint_directory().join("control.json")).is_ok_and(|endpoint| {
        TcpStream::connect_timeout(&endpoint_address(&endpoint), CONNECT_TIMEOUT).is_ok()
    })
}

fn send_to(request: &Request, path: &Path) -> Result<Response, String> {
    let endpoint = read_endpoint(path)?;
    let mut stream = TcpStream::connect_timeout(&endpoint_address(&endpoint), CONNECT_TIMEOUT)
        .map_err(|error| {
            format!(
                "The vEdit window is not reachable ({error}). Start vEdit and try again. The request was not sent."
            )
        })?;
    configure_stream(&stream).map_err(|error| {
        format!("Could not configure the connection: {error}. The request was not sent.")
    })?;
    let packet = Outgoing {
        version: PROTOCOL_VERSION,
        token: &endpoint.token,
        request,
    };
    let mut bytes = serde_json::to_vec(&packet)
        .map_err(|error| format!("Could not encode the control request: {error}"))?;
    if bytes.len() + 1 > MAX_REQUEST_BYTES {
        return Err(format!(
            "The encoded control request exceeds {MAX_REQUEST_BYTES} bytes. The request was not sent. Reduce its payload or use smaller independent edits. Keep a review's intended annotations together: each review replaces the previous one."
        ));
    }
    bytes.push(b'\n');
    stream.write_all(&bytes).map_err(|error| {
        format!(
            "The connection failed while sending the request: {error}. Its outcome is unknown. Inspect vEdit's status before retrying a change."
        )
    })?;
    let response = read_frame_with_timeout(&mut stream, MAX_RESPONSE_BYTES, request_timeout(request) + Duration::from_secs(1)).map_err(|error| {
        format!(
            "vEdit did not return a complete result: {error}. The request may have been applied. Inspect vEdit's status before retrying a change."
        )
    })?;
    from_unique_json(&response).map_err(|error| {
        format!(
            "vEdit returned an invalid result: {error}. The request may have been applied. Inspect vEdit's status before retrying a change."
        )
    })
}

fn endpoint_address(endpoint: &Endpoint) -> SocketAddr {
    SocketAddr::from((Ipv4Addr::LOCALHOST, endpoint.port))
}

fn read_endpoint(path: &Path) -> Result<Endpoint, String> {
    let file = fs::File::open(path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            "No vEdit control endpoint exists. The request was not sent. Start vEdit, or use open/workspace to start the sibling executable.".into()
        } else {
            format!("Could not read the vEdit control endpoint: {error}")
        }
    })?;
    let mut bytes = Vec::new();
    file.take(4097)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Could not read the vEdit control endpoint: {error}"))?;
    if bytes.len() > 4096 {
        return Err("The vEdit control endpoint is invalid. Restart vEdit.".into());
    }
    let endpoint: Endpoint = from_unique_json(&bytes)
        .map_err(|_| "The vEdit control endpoint is invalid. Restart vEdit.".to_owned())?;
    if endpoint.version != PROTOCOL_VERSION {
        return Err(format!(
            "vEdit's control protocol is version {}; this CLI supports version {PROTOCOL_VERSION}. Use the CLI packaged with the running editor.",
            endpoint.version
        ));
    }
    if endpoint.port == 0
        || endpoint.token.len() != 64
        || !endpoint.token.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("The vEdit control endpoint is invalid. Restart vEdit.".into());
    }
    Ok(endpoint)
}

fn serve_connection(
    mut stream: TcpStream,
    token: &str,
    sender: &mpsc::SyncSender<Envelope>,
    stop: &AtomicBool,
    ctx: &egui::Context,
) {
    if configure_stream(&stream).is_err() {
        return;
    }
    let response = match read_frame(&mut stream, MAX_REQUEST_BYTES) {
        Ok(frame) => match decode_request(&frame, token) {
            Ok(request) => {
                let timeout = request_timeout(&request);
                let (reply, receiver) = mpsc::channel();
                match sender.try_send(Envelope { request, reply }) {
                    Ok(()) => {
                        ctx.request_repaint();
                        wait_for_application(receiver, stop, timeout)
                    }
                    Err(mpsc::TrySendError::Full(_)) => Response::failure(
                        "busy",
                        "vEdit's control queue is full. This request was not applied.",
                    ),
                    Err(mpsc::TrySendError::Disconnected(_)) => Response::failure(
                        "server_closed",
                        "The vEdit window closed before the request was queued. This request was not applied.",
                    ),
                }
            }
            Err(response) => response,
        },
        Err(error) => Response::failure(
            if error.kind() == io::ErrorKind::InvalidData {
                "request_too_large"
            } else {
                "invalid_request"
            },
            format!("Could not read the control request: {error}. It was not applied."),
        ),
    };
    write_response(&mut stream, &response);
}

fn decode_request(frame: &[u8], token: &str) -> Result<Request, Response> {
    let incoming: Incoming = from_unique_json(frame).map_err(|error| {
        Response::failure(
            "invalid_request",
            format!("The control request is not valid JSON: {error}. It was not applied."),
        )
    })?;
    if !tokens_equal(incoming.token.as_bytes(), token.as_bytes()) {
        return Err(Response::failure(
            "unauthorized",
            "The control token is invalid. This request was not applied.",
        ));
    }
    if incoming.version != PROTOCOL_VERSION {
        return Err(Response::failure(
            "unsupported_protocol",
            format!(
                "Use control protocol version {PROTOCOL_VERSION}. This request was not applied."
            ),
        ));
    }
    serde_json::from_value(incoming.request).map_err(|error| {
        Response::failure(
            "invalid_request",
            format!("The control operation is invalid: {error}. It was not applied."),
        )
    })
}

fn request_timeout(request: &Request) -> Duration {
    if matches!(
        request,
        Request::GitBranches { .. }
            | Request::GitHistory { .. }
            | Request::GitSwitchBranch { .. }
            | Request::ChangeReview { .. }
    ) {
        Duration::from_secs(50)
    } else {
        REQUEST_TIMEOUT
    }
}

fn wait_for_application(
    receiver: mpsc::Receiver<Response>,
    stop: &AtomicBool,
    timeout: Duration,
) -> Response {
    let deadline = Instant::now() + timeout;
    loop {
        if stop.load(Ordering::Acquire) {
            return Response::failure(
                "server_closed",
                "vEdit closed before returning a result. The request may have been applied.",
            );
        }
        match receiver.recv_timeout(Duration::from_millis(50)) {
            Ok(response) => return response,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Response::failure(
                    "server_closed",
                    "The request handler closed before returning a result. The request may have been applied.",
                );
            }
            Err(mpsc::RecvTimeoutError::Timeout) if Instant::now() >= deadline => {
                return Response::failure(
                    "timeout",
                    format!(
                        "vEdit did not return a result within {} seconds. Its outcome is unknown. Inspect git-status/status before retrying a change.",
                        timeout.as_secs()
                    ),
                );
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

fn configure_stream(stream: &TcpStream) -> io::Result<()> {
    // On Windows an accepted socket inherits the listener's nonblocking mode.
    // A JSON frame may span several packets; wait for the next packet instead
    // of treating WouldBlock as an invalid (or partially delivered) request.
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(REQUEST_TIMEOUT))?;
    stream.set_write_timeout(Some(REQUEST_TIMEOUT))?;
    stream.set_nodelay(true)
}

fn read_frame(stream: &mut TcpStream, limit: usize) -> io::Result<Vec<u8>> {
    read_frame_with_timeout(stream, limit, REQUEST_TIMEOUT)
}

fn read_frame_with_timeout(
    stream: &mut TcpStream,
    limit: usize,
    timeout: Duration,
) -> io::Result<Vec<u8>> {
    let deadline = Instant::now() + timeout;
    let mut frame = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "the complete JSON frame was not received within ten seconds",
            ));
        }
        stream.set_read_timeout(Some(remaining))?;
        let capacity = buffer.len().min(limit + 1 - frame.len());
        let count = stream.read(&mut buffer[..capacity])?;
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the JSON frame must end with a newline",
            ));
        }
        if let Some(newline) = buffer[..count].iter().position(|byte| *byte == b'\n') {
            if frame.len() + newline + 1 > limit {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("JSON frame exceeds {limit} bytes"),
                ));
            }
            frame.extend_from_slice(&buffer[..newline]);
            return Ok(frame);
        }
        frame.extend_from_slice(&buffer[..count]);
        if frame.len() >= limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("JSON frame exceeds {limit} bytes"),
            ));
        }
    }
}

fn write_response(stream: &mut TcpStream, response: &Response) {
    let Ok(mut bytes) = serde_json::to_vec(response) else {
        return;
    };
    if bytes.len() + 1 > MAX_RESPONSE_BYTES {
        bytes = serde_json::to_vec(&Response::failure(
            "response_too_large",
            "The result is too large. Request a smaller range or fewer tabs. The original operation may already have been applied.",
        ))
        .unwrap_or_default();
    }
    bytes.push(b'\n');
    let _ = stream.write_all(&bytes);
}

fn tokens_equal(first: &[u8], second: &[u8]) -> bool {
    if first.len() != second.len() {
        return false;
    }
    first
        .iter()
        .zip(second)
        .fold(0_u8, |difference, (a, b)| difference | (a ^ b))
        == 0
}

fn private_directory(directory: &Path) -> io::Result<()> {
    fs::create_dir_all(directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn write_endpoint(path: &Path, endpoint: &Endpoint) -> io::Result<()> {
    let temporary = path.with_extension(format!("{}.tmp", endpoint.token));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        #[cfg(windows)]
        protect_endpoint_file(&temporary)?;
        serde_json::to_writer(&mut file, endpoint)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        drop(file);
        replace_file(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

#[cfg(not(windows))]
fn replace_file(from: &Path, to: &Path) -> io::Result<()> {
    fs::rename(from, to)
}

#[cfg(windows)]
fn replace_file(from: &Path, to: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, replacement: *const u16, flags: u32) -> i32;
    }
    let from: Vec<_> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<_> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    // Both pointers refer to owned, NUL-terminated buffers for this call.
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 0x1 | 0x8) } == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(windows)]
fn protect_endpoint_file(path: &Path) -> io::Result<()> {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
            descriptor: *const u16,
            revision: u32,
            result: *mut *mut c_void,
            size: *mut u32,
        ) -> i32;
        fn SetFileSecurityW(path: *const u16, information: u32, descriptor: *const c_void) -> i32;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LocalFree(memory: *mut c_void) -> *mut c_void;
    }
    // A protected DACL grants access only to the file owner and Windows itself.
    // Apply it before writing the secret, including in custom control directories.
    let descriptor: Vec<_> = "D:P(A;;FA;;;OW)(A;;FA;;;SY)"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let path: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut security = std::ptr::null_mut();
    // Windows allocates this descriptor; LocalFree below releases it once.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            descriptor.as_ptr(),
            1,
            &mut security,
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    // Set only the DACL, with inheritance disabled; retain the actual file owner.
    let succeeded = unsafe { SetFileSecurityW(path.as_ptr(), 0x4 | 0x8000_0000, security) };
    let error = (succeeded == 0).then(io::Error::last_os_error);
    unsafe { LocalFree(security) };
    error.map_or(Ok(()), Err)
}

fn random_token() -> io::Result<String> {
    let mut bytes = [0_u8; 32];
    fill_random(&mut bytes)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(windows)]
fn fill_random(bytes: &mut [u8]) -> io::Result<()> {
    use std::ffi::c_void;
    #[link(name = "bcrypt")]
    unsafe extern "system" {
        fn BCryptGenRandom(algorithm: *mut c_void, buffer: *mut u8, bytes: u32, flags: u32) -> i32;
    }
    // BCRYPT_USE_SYSTEM_PREFERRED_RNG asks Windows for its system CSPRNG.
    let status = unsafe {
        BCryptGenRandom(
            std::ptr::null_mut(),
            bytes.as_mut_ptr(),
            bytes.len() as u32,
            0x2,
        )
    };
    if status < 0 {
        Err(io::Error::other(format!(
            "Windows random generator failed (NTSTATUS {status:#x})"
        )))
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn fill_random(bytes: &mut [u8]) -> io::Result<()> {
    fs::File::open("/dev/urandom")?.read_exact(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::net::Shutdown;

    struct Fixture {
        directory: PathBuf,
        server: Option<Server>,
    }

    impl Fixture {
        fn new() -> Self {
            let directory = std::env::temp_dir().join(format!(
                "vedit-control-test-{}-{}",
                std::process::id(),
                random_token().unwrap()
            ));
            let server = Server::start_in(egui::Context::default(), directory.clone()).unwrap();
            Self {
                directory,
                server: Some(server),
            }
        }

        fn path(&self) -> PathBuf {
            self.directory.join("control.json")
        }

        fn server(&self) -> &Server {
            self.server.as_ref().unwrap()
        }

        fn endpoint(&self) -> Endpoint {
            read_endpoint(&self.path()).unwrap()
        }

        fn receive(&self) -> Envelope {
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                if let Some(envelope) = self.server().try_recv() {
                    return envelope;
                }
                assert!(Instant::now() < deadline, "request did not reach the GUI");
                thread::sleep(Duration::from_millis(5));
            }
        }

        fn raw_request(&self, bytes: &[u8]) -> Response {
            let mut socket = TcpStream::connect(endpoint_address(&self.endpoint())).unwrap();
            configure_stream(&socket).unwrap();
            let _ = socket.write_all(bytes);
            socket.shutdown(Shutdown::Write).unwrap();
            serde_json::from_slice(&read_frame(&mut socket, MAX_RESPONSE_BYTES).unwrap()).unwrap()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            drop(self.server.take());
            let _ = fs::remove_dir_all(&self.directory);
        }
    }

    fn error_code(response: Response) -> String {
        assert!(!response.ok);
        response.error.unwrap().code
    }

    #[test]
    fn round_trip_waits_for_the_gui_application_result() {
        let fixture = Fixture::new();
        let path = fixture.path();
        let client = thread::spawn(move || send_to(&Request::Status {}, &path));
        let envelope = fixture.receive();
        assert!(matches!(envelope.request, Request::Status {}));
        envelope
            .reply
            .send(Response::success(json!({"active_tab_id": 42})))
            .unwrap();
        let response = client.join().unwrap().unwrap();
        assert!(response.ok);
        assert_eq!(response.result["active_tab_id"], 42);
    }

    #[test]
    fn invalid_tokens_do_not_reach_the_gui() {
        let fixture = Fixture::new();
        let mut packet = serde_json::to_vec(&json!({
            "version": PROTOCOL_VERSION,
            "token": "bad-token",
            "request": {"op": "status"}
        }))
        .unwrap();
        packet.push(b'\n');
        assert_eq!(error_code(fixture.raw_request(&packet)), "unauthorized");
        assert!(fixture.server().try_recv().is_none());
    }

    #[test]
    fn version_and_operation_are_checked_before_gui_dispatch() {
        let fixture = Fixture::new();
        let endpoint = fixture.endpoint();
        for (version, request, expected) in [
            (
                PROTOCOL_VERSION + 1,
                json!({"op": "status"}),
                "unsupported_protocol",
            ),
            (
                PROTOCOL_VERSION,
                json!({"op": "nonexistent"}),
                "invalid_request",
            ),
        ] {
            let mut packet = serde_json::to_vec(&json!({
                "version": version, "token": endpoint.token, "request": request
            }))
            .unwrap();
            packet.push(b'\n');
            assert_eq!(error_code(fixture.raw_request(&packet)), expected);
        }
        assert!(fixture.server().try_recv().is_none());
    }

    #[test]
    fn malformed_json_and_incomplete_frames_are_rejected() {
        let fixture = Fixture::new();
        assert_eq!(error_code(fixture.raw_request(b"{\n")), "invalid_request");
        assert_eq!(error_code(fixture.raw_request(b"{}")), "invalid_request");
        assert!(fixture.server().try_recv().is_none());
    }

    #[test]
    fn duplicate_wrapper_and_nested_request_keys_never_reach_the_gui() {
        let fixture = Fixture::new();
        let token = fixture.endpoint().token;
        let packets = [
            format!(
                "{{\"version\":1,\"token\":\"{token}\",\"token\":\"{token}\",\"request\":{{\"op\":\"status\"}}}}\n"
            ),
            format!(
                "{{\"version\":1,\"token\":\"{token}\",\"request\":{{\"op\":\"status\",\"op\":\"raise\"}}}}\n"
            ),
            format!(
                "{{\"version\":1,\"token\":\"{token}\",\"request\":{{\"op\":\"open\",\"files\":[{{\"path\":\"first.md\",\"path\":\"second.md\"}}]}}}}\n"
            ),
            format!(
                "{{\"version\":1,\"token\":\"{token}\",\"request\":{{\"op\":\"focus\",\"tab_id\":1,\"location\":{{\"line\":1,\"line\":2}}}}}}\n"
            ),
            format!(
                "{{\"version\":1,\"token\":\"{token}\",\"request\":{{\"op\":\"status\",\"\\u006f\\u0070\":\"raise\"}}}}\n"
            ),
        ];
        for packet in packets {
            let response = fixture.raw_request(packet.as_bytes());
            assert_eq!(response.error.as_ref().unwrap().code, "invalid_request");
            assert!(
                response
                    .error
                    .unwrap()
                    .message
                    .contains("Duplicate JSON object key")
            );
        }
        assert!(fixture.server().try_recv().is_none());
    }

    #[test]
    fn strict_json_preserves_all_value_types_and_repeated_keys_in_other_objects() {
        let json = br#"{"bool":true,"null":null,"array":[-5,18446744073709551615,2.5,"text",{"a":1},{"a":2}]}"#;
        let value: serde_json::Value = from_unique_json(json).unwrap();
        assert_eq!(
            value,
            serde_json::from_slice::<serde_json::Value>(json).unwrap()
        );
        assert!(from_unique_json::<serde_json::Value>(br#"{"a":1,"a":1}"#).is_err());
    }

    #[test]
    fn oversized_request_is_rejected_before_deserialization() {
        let fixture = Fixture::new();
        let bytes = vec![b'x'; MAX_REQUEST_BYTES + 1];
        assert_eq!(error_code(fixture.raw_request(&bytes)), "request_too_large");
        assert!(fixture.server().try_recv().is_none());
    }

    #[test]
    fn fragmented_request_waits_for_the_complete_frame_before_dispatch() {
        let fixture = Fixture::new();
        let endpoint = fixture.endpoint();
        let mut socket = TcpStream::connect(endpoint_address(&endpoint)).unwrap();
        configure_stream(&socket).unwrap();
        let packet = serde_json::to_vec(&Outgoing {
            version: PROTOCOL_VERSION,
            token: &endpoint.token,
            request: &Request::Status {},
        })
        .unwrap();
        socket.write_all(&packet[..12]).unwrap();
        thread::sleep(Duration::from_millis(50));
        assert!(fixture.server().try_recv().is_none());
        socket.write_all(&packet[12..]).unwrap();
        socket.write_all(b"\n").unwrap();
        let envelope = fixture.receive();
        assert!(matches!(envelope.request, Request::Status {}));
        envelope
            .reply
            .send(Response::success(serde_json::json!({"complete": true})))
            .unwrap();
        let response: Response =
            serde_json::from_slice(&read_frame(&mut socket, MAX_RESPONSE_BYTES).unwrap()).unwrap();
        assert!(response.ok);
        assert!(fixture.server().try_recv().is_none());
    }

    #[test]
    fn missing_dead_and_incompatible_endpoints_have_actionable_errors() {
        let fixture = Fixture::new();
        let missing = fixture.directory.join("missing.json");
        assert!(
            send_to(&Request::Status {}, &missing)
                .unwrap_err()
                .contains("Start vEdit")
        );

        let mut endpoint = fixture.endpoint();
        endpoint.version += 1;
        let incompatible = fixture.directory.join("incompatible.json");
        write_endpoint(&incompatible, &endpoint).unwrap();
        assert!(
            send_to(&Request::Status {}, &incompatible)
                .unwrap_err()
                .contains("CLI packaged with the running editor")
        );

        let temporary_listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        endpoint.version = PROTOCOL_VERSION;
        endpoint.port = temporary_listener.local_addr().unwrap().port();
        drop(temporary_listener);
        let dead = fixture.directory.join("dead.json");
        write_endpoint(&dead, &endpoint).unwrap();
        assert!(
            send_to(&Request::Status {}, &dead)
                .unwrap_err()
                .contains("not sent")
        );
    }

    #[test]
    fn live_server_cannot_be_accidentally_replaced() {
        let fixture = Fixture::new();
        let original = fixture.endpoint();
        let attempt = Server::start_in(egui::Context::default(), fixture.directory.clone());
        assert!(attempt.is_err());
        assert_eq!(fixture.endpoint().token, original.token);
    }

    #[test]
    fn stale_endpoint_is_replaced_when_its_listener_is_gone() {
        let mut fixture = Fixture::new();
        let old = fixture.endpoint();
        drop(fixture.server.take());
        write_endpoint(&fixture.path(), &old).unwrap();
        let replacement =
            Server::start_in(egui::Context::default(), fixture.directory.clone()).unwrap();
        fixture.server = Some(replacement);
        assert_ne!(fixture.endpoint().token, old.token);
    }

    #[test]
    fn network_concurrency_is_bounded_without_extra_gui_dispatch() {
        let fixture = Fixture::new();
        let endpoint = fixture.endpoint();
        let mut packet = serde_json::to_vec(&Outgoing {
            version: PROTOCOL_VERSION,
            token: &endpoint.token,
            request: &Request::Status {},
        })
        .unwrap();
        packet.push(b'\n');

        // Submit every authenticated frame before waiting for admission. Spawning
        // and awaiting one client at a time tests scheduler latency as well as the
        // connection limit, and leaves earlier clients approaching their timeout.
        let deadline = Instant::now() + REQUEST_TIMEOUT;
        let mut sockets = Vec::new();
        for _ in 0..MAX_CONNECTIONS {
            let mut socket =
                TcpStream::connect_timeout(&endpoint_address(&endpoint), CONNECT_TIMEOUT).unwrap();
            configure_stream(&socket).unwrap();
            socket.write_all(&packet).unwrap();
            sockets.push(socket);
        }
        let mut requests = Vec::new();
        for index in 0..MAX_CONNECTIONS {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let envelope = fixture
                .server()
                .receiver
                .recv_timeout(remaining)
                .unwrap_or_else(|error| {
                    panic!("only {index} of {MAX_CONNECTIONS} requests reached the GUI: {error}")
                });
            assert!(matches!(envelope.request, Request::Status {}));
            requests.push(envelope);
        }

        // The admitted requests remain in flight while the ninth connection is
        // refused. None has received an application reply or released its slot.
        assert!(send_to(&Request::Status {}, &fixture.path()).is_err());
        assert!(fixture.server().try_recv().is_none());
        for envelope in requests {
            envelope
                .reply
                .send(Response::success(json!({"ready": true})))
                .unwrap();
        }
        for mut socket in sockets {
            let bytes = read_frame(&mut socket, MAX_RESPONSE_BYTES).unwrap();
            let response: Response = from_unique_json(&bytes).unwrap();
            assert!(response.ok);
            assert_eq!(response.result["ready"], true);
        }
    }

    #[test]
    fn clean_shutdown_removes_only_the_owned_endpoint() {
        let mut fixture = Fixture::new();
        drop(fixture.server.take());
        assert!(!fixture.path().exists());
        let replacement =
            Server::start_in(egui::Context::default(), fixture.directory.clone()).unwrap();
        let replacement_token = read_endpoint(&fixture.path()).unwrap().token;
        fixture.server = Some(replacement);
        assert_eq!(fixture.endpoint().token, replacement_token);
    }

    #[test]
    fn disconnected_gui_reports_unknown_outcome() {
        let fixture = Fixture::new();
        let path = fixture.path();
        let client = thread::spawn(move || send_to(&Request::Status {}, &path));
        drop(fixture.receive());
        let response = client.join().unwrap().unwrap();
        assert_eq!(response.error.unwrap().code, "server_closed");
    }

    #[test]
    fn stopping_server_wakes_clients_waiting_for_application() {
        let mut fixture = Fixture::new();
        let path = fixture.path();
        let client = thread::spawn(move || send_to(&Request::Status {}, &path));
        let _envelope = fixture.receive();
        drop(fixture.server.take());
        let response = client.join().unwrap().unwrap();
        assert_eq!(response.error.unwrap().code, "server_closed");
    }

    #[test]
    fn idle_listener_sleeps_until_a_client_connects() {
        let fixture = Fixture::new();
        let wakeups = fixture.server().wakeups.clone();
        let before = wakeups.load(Ordering::Relaxed);
        thread::sleep(Duration::from_millis(500));
        let idle = wakeups.load(Ordering::Relaxed) - before;
        eprintln!("idle control listener wakeups in 500 ms: {idle}");
        assert_eq!(idle, 0, "an idle control listener must not poll");
        // A client is still accepted promptly without polling.
        let path = fixture.path();
        let client = thread::spawn(move || send_to(&Request::Status {}, &path));
        let envelope = fixture.receive();
        envelope
            .reply
            .send(Response::success(json!({"awake": true})))
            .unwrap();
        assert!(client.join().unwrap().unwrap().ok);
        let started = Instant::now();
        drop(fixture);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "stopping must wake the blocked listener"
        );
    }

    #[test]
    fn cryptographic_tokens_are_unique_and_compare_exactly() {
        let first = random_token().unwrap();
        let second = random_token().unwrap();
        assert_eq!(first.len(), 64);
        assert!(first.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_ne!(first, second);
        assert!(tokens_equal(first.as_bytes(), first.as_bytes()));
        assert!(!tokens_equal(first.as_bytes(), second.as_bytes()));
        assert!(!tokens_equal(first.as_bytes(), b""));
    }

    #[cfg(unix)]
    #[test]
    fn endpoint_and_directory_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = Fixture::new();
        assert_eq!(
            fs::metadata(fixture.path()).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(&fixture.directory)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
}
