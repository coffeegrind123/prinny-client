// MSC4544's local transport: apps (games, media players) send rich presence
// to the running Matrix client over a WebSocket, and the client merges it
// into the user's `m.rpc` profile field.
//
// https://github.com/matrix-org/matrix-spec-proposals/pull/4544, "Data transport":
//
//   - a unix domain socket at /tmp/.[client_name]_[opaque_id]/presence.sock
//   - ws://localhost:{port}/presence on the lowest free port in 44190-45190
//
// Desktop only, and only while the frontend's `publishRichPresence` setting is
// on. Messages are forwarded to the webview unparsed beyond "is a JSON
// object"; which apps may publish (MSC4544 requires a per-app allow list that
// defaults to deny) and the merge into the profile field live in the webapp,
// where they can be unit-tested and changed without a native rebuild.
//
// Who may connect:
//
//   - The TCP listener binds 127.0.0.1 only.
//   - Any handshake carrying an `Origin` header is refused. Browsers always
//     send one on a WebSocket handshake and native apps do not, so this is what
//     stops an arbitrary web page from publishing presence as the user through
//     ws://localhost. `Host` must also name the loopback, against DNS rebinding.
//   - The unix socket's directory is created 0700 and refused if it exists with
//     any group/other permission, and the socket itself is 0600: /tmp is shared
//     between users, unlike the XDG_RUNTIME_DIR the Discord bridge uses, and
//     the MSC fixes the /tmp location. `opaque_id` is an HMAC of the user ID
//     with a per-installation salt, computed by the webview, so the path does
//     not reveal who is logged in and cannot be predicted to squat it.
//
// The WebSocket framing is implemented here (RFC 6455, server side, text
// messages only) rather than pulled in as a crate: it is a few hundred lines
// with test vectors, and it keeps Cargo.lock and the Android build untouched.

// The framing and handshake helpers are only reached from the desktop server.
#![cfg_attr(mobile, allow(dead_code))]

use std::sync::Mutex;

use serde::Serialize;

/// Event carrying one message from an app: `{ connection, message }`.
#[cfg(not(mobile))]
pub const MESSAGE_EVENT: &str = "app-presence-message";

/// MSC4544: the lowest free port in this range.
pub const PORT_FIRST: u16 = 44190;
pub const PORT_LAST: u16 = 45190;

/// Largest message accepted from an app. An add_presence is well under 4 KiB.
pub const MAX_MESSAGE: usize = 64 * 1024;
/// Largest HTTP handshake request accepted.
pub const MAX_HANDSHAKE: usize = 8 * 1024;
/// Connections served at once; further ones are closed straight away.
pub const MAX_CONNECTIONS: usize = 16;

#[derive(Serialize, Clone, Debug)]
pub struct AppPresenceStarted {
    /// The TCP port bound, if any.
    pub port: Option<u16>,
    /// The unix socket path bound, if any (never on Windows).
    pub socket: Option<String>,
}

// ── SHA-1 and base64, for Sec-WebSocket-Accept ──────────────────────────────

/// SHA-1 (FIPS 180-4). Only used for the WebSocket handshake, where RFC 6455
/// mandates it; it carries no security weight there.
pub fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];

    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut message = data.to_vec();
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in message.chunks(64) {
        let mut w = [0u32; 80];
        for (i, word) in chunk.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }

        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }

    let mut out = [0u8; 20];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

/// Standard base64 with padding.
pub fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// RFC 6455 §4.2.2: base64(SHA-1(key + GUID)).
pub fn accept_key(key: &str) -> String {
    const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
    base64(&sha1(format!("{key}{GUID}").as_bytes()))
}

// ── Handshake ───────────────────────────────────────────────────────────────

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Transport {
    Tcp,
    Unix,
}

#[derive(Debug, PartialEq, Eq)]
pub enum HandshakeError {
    /// Not a complete request yet.
    Incomplete,
    /// Over MAX_HANDSHAKE.
    TooLarge,
    /// Answer 400.
    Bad(&'static str),
    /// Answer 403.
    Forbidden(&'static str),
    /// Answer 404.
    NotFound,
}

/// The `Sec-WebSocket-Key` of a valid handshake request.
///
/// `buf` holds what has been read so far; `Incomplete` asks for more.
pub fn parse_handshake(buf: &[u8], transport: Transport) -> Result<String, HandshakeError> {
    let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
        return Err(if buf.len() > MAX_HANDSHAKE {
            HandshakeError::TooLarge
        } else {
            HandshakeError::Incomplete
        });
    };
    if end > MAX_HANDSHAKE {
        return Err(HandshakeError::TooLarge);
    }
    let head = std::str::from_utf8(&buf[..end]).map_err(|_| HandshakeError::Bad("not utf-8"))?;
    let mut lines = head.split("\r\n");

    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split(' ');
    let (method, target, version) = (parts.next(), parts.next(), parts.next());
    if method != Some("GET") || version != Some("HTTP/1.1") {
        return Err(HandshakeError::Bad("expected GET ... HTTP/1.1"));
    }
    let path = target
        .unwrap_or_default()
        .split('?')
        .next()
        .unwrap_or_default();
    if path != "/presence" && path != "/" {
        return Err(HandshakeError::NotFound);
    }

    let mut key = None;
    let mut upgrade = false;
    let mut connection_upgrade = false;
    let mut version_13 = false;
    let mut host = None;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            return Err(HandshakeError::Bad("malformed header"));
        };
        let value = value.trim();
        match name.trim().to_ascii_lowercase().as_str() {
            "origin" => {
                return Err(HandshakeError::Forbidden(
                    "browser origins are not accepted",
                ))
            }
            "upgrade" => upgrade = value.eq_ignore_ascii_case("websocket"),
            "connection" => {
                connection_upgrade = value
                    .split(',')
                    .any(|token| token.trim().eq_ignore_ascii_case("upgrade"))
            }
            "sec-websocket-version" => version_13 = value == "13",
            "sec-websocket-key" => key = Some(value.to_owned()),
            "host" => host = Some(value.to_ascii_lowercase()),
            _ => {}
        }
    }

    if !upgrade || !connection_upgrade {
        return Err(HandshakeError::Bad("not a websocket upgrade"));
    }
    if !version_13 {
        return Err(HandshakeError::Bad("websocket version 13 required"));
    }
    let key = key.ok_or(HandshakeError::Bad("missing Sec-WebSocket-Key"))?;
    // 16 random bytes, base64: always 24 characters ending in "==".
    if key.len() != 24 || !key.ends_with("==") {
        return Err(HandshakeError::Bad("malformed Sec-WebSocket-Key"));
    }
    if transport == Transport::Tcp && !host.as_deref().is_some_and(is_loopback_host) {
        return Err(HandshakeError::Forbidden("Host must be the loopback"));
    }
    Ok(key)
}

/// `localhost`, `127.0.0.1` or `[::1]`, with or without a port.
fn is_loopback_host(host: &str) -> bool {
    let name = if let Some(rest) = host.strip_prefix('[') {
        rest.split(']').next().unwrap_or_default()
    } else {
        host.rsplit_once(':').map_or(host, |(name, port)| {
            if port.chars().all(|c| c.is_ascii_digit()) {
                name
            } else {
                host
            }
        })
    };
    matches!(name, "localhost" | "127.0.0.1" | "::1")
}

pub fn handshake_response(key: &str) -> String {
    format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
        accept_key(key)
    )
}

pub fn error_response(err: &HandshakeError) -> String {
    let (status, reason) = match err {
        HandshakeError::Forbidden(reason) => ("403 Forbidden", *reason),
        HandshakeError::NotFound => ("404 Not Found", "not found"),
        HandshakeError::TooLarge => ("431 Request Header Fields Too Large", "too large"),
        HandshakeError::Bad(reason) => ("400 Bad Request", *reason),
        HandshakeError::Incomplete => ("400 Bad Request", "incomplete request"),
    };
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reason}",
        reason.len()
    )
}

// ── Frames ──────────────────────────────────────────────────────────────────

pub const OP_CONTINUATION: u8 = 0x0;
pub const OP_TEXT: u8 = 0x1;
pub const OP_BINARY: u8 = 0x2;
pub const OP_CLOSE: u8 = 0x8;
pub const OP_PING: u8 = 0x9;
pub const OP_PONG: u8 = 0xA;

pub const CLOSE_NORMAL: u16 = 1000;
pub const CLOSE_PROTOCOL: u16 = 1002;
pub const CLOSE_UNSUPPORTED: u16 = 1003;
pub const CLOSE_INVALID: u16 = 1007;
pub const CLOSE_TOO_BIG: u16 = 1009;

#[derive(Debug, PartialEq, Eq)]
pub struct Frame {
    pub fin: bool,
    pub opcode: u8,
    pub payload: Vec<u8>,
}

/// One client frame from the start of `buf`: `Ok(None)` when more bytes are
/// needed, otherwise the frame and how many bytes it used. Errors carry the
/// close code to answer with.
pub fn decode_frame(buf: &[u8]) -> Result<Option<(Frame, usize)>, u16> {
    if buf.len() < 2 {
        return Ok(None);
    }
    let fin = buf[0] & 0x80 != 0;
    if buf[0] & 0x70 != 0 {
        // No extension was negotiated, so reserved bits must be clear.
        return Err(CLOSE_PROTOCOL);
    }
    let opcode = buf[0] & 0x0F;
    let masked = buf[1] & 0x80 != 0;
    if !masked {
        // RFC 6455 §5.1: a server MUST close on an unmasked client frame.
        return Err(CLOSE_PROTOCOL);
    }

    let mut offset = 2;
    let len = match buf[1] & 0x7F {
        126 => {
            if buf.len() < 4 {
                return Ok(None);
            }
            offset = 4;
            u16::from_be_bytes([buf[2], buf[3]]) as u64
        }
        127 => {
            if buf.len() < 10 {
                return Ok(None);
            }
            offset = 10;
            let mut bytes = [0u8; 8];
            bytes.copy_from_slice(&buf[2..10]);
            u64::from_be_bytes(bytes)
        }
        n => n as u64,
    };
    if opcode >= OP_CLOSE && (len > 125 || !fin) {
        return Err(CLOSE_PROTOCOL);
    }
    if len > MAX_MESSAGE as u64 {
        return Err(CLOSE_TOO_BIG);
    }
    let len = len as usize;

    if buf.len() < offset + 4 + len {
        return Ok(None);
    }
    let mask = [
        buf[offset],
        buf[offset + 1],
        buf[offset + 2],
        buf[offset + 3],
    ];
    offset += 4;
    let payload = buf[offset..offset + len]
        .iter()
        .enumerate()
        .map(|(i, byte)| byte ^ mask[i % 4])
        .collect();
    Ok(Some((
        Frame {
            fin,
            opcode,
            payload,
        },
        offset + len,
    )))
}

/// A server frame: never masked (RFC 6455 §5.1).
pub fn encode_frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 10);
    out.push(0x80 | opcode);
    match payload.len() {
        len @ 0..=125 => out.push(len as u8),
        len @ 126..=0xFFFF => {
            out.push(126);
            out.extend_from_slice(&(len as u16).to_be_bytes());
        }
        len => {
            out.push(127);
            out.extend_from_slice(&(len as u64).to_be_bytes());
        }
    }
    out.extend_from_slice(payload);
    out
}

pub fn close_frame(code: u16) -> Vec<u8> {
    encode_frame(OP_CLOSE, &code.to_be_bytes())
}

/// What a connection should do after one frame.
#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    /// Nothing to do yet (a fragment was buffered, or a pong arrived).
    Continue,
    /// A complete text message.
    Message(String),
    /// Send these bytes back (a pong).
    Reply(Vec<u8>),
    /// Send this close frame and stop.
    Close(u16),
}

/// Reassembles fragmented messages and answers control frames.
#[derive(Default)]
pub struct MessageAssembler {
    partial: Option<Vec<u8>>,
}

impl MessageAssembler {
    pub fn push(&mut self, frame: Frame) -> Step {
        match frame.opcode {
            OP_PING => Step::Reply(encode_frame(OP_PONG, &frame.payload)),
            OP_PONG => Step::Continue,
            OP_CLOSE => Step::Close(CLOSE_NORMAL),
            OP_BINARY => Step::Close(CLOSE_UNSUPPORTED),
            OP_TEXT => {
                if self.partial.is_some() {
                    return Step::Close(CLOSE_PROTOCOL);
                }
                if frame.fin {
                    Self::finish(frame.payload)
                } else {
                    self.partial = Some(frame.payload);
                    Step::Continue
                }
            }
            OP_CONTINUATION => {
                let Some(mut partial) = self.partial.take() else {
                    return Step::Close(CLOSE_PROTOCOL);
                };
                if partial.len() + frame.payload.len() > MAX_MESSAGE {
                    return Step::Close(CLOSE_TOO_BIG);
                }
                partial.extend_from_slice(&frame.payload);
                if frame.fin {
                    Self::finish(partial)
                } else {
                    self.partial = Some(partial);
                    Step::Continue
                }
            }
            _ => Step::Close(CLOSE_PROTOCOL),
        }
    }

    fn finish(bytes: Vec<u8>) -> Step {
        match String::from_utf8(bytes) {
            Ok(text) => Step::Message(text),
            Err(_) => Step::Close(CLOSE_INVALID),
        }
    }
}

/// `opaque_id` as the webview sends it: lowercase hex, 16 to 64 characters.
/// Anything else would let the caller steer the socket path.
pub fn valid_opaque_id(id: &str) -> bool {
    (16..=64).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[cfg(not(mobile))]
mod imp {
    use super::*;

    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::sync::Arc;

    use serde_json::{json, Value};
    use tauri::{AppHandle, Emitter, Runtime};
    use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::{oneshot, watch};

    static NEXT_CONNECTION_ID: AtomicU64 = AtomicU64::new(1);

    async fn serve<S, R>(
        mut stream: S,
        transport: Transport,
        app: AppHandle<R>,
        open: Arc<AtomicUsize>,
    ) where
        S: AsyncRead + AsyncWrite + Unpin,
        R: Runtime,
    {
        let connection = NEXT_CONNECTION_ID.fetch_add(1, Ordering::Relaxed);
        let mut buf: Vec<u8> = Vec::with_capacity(1024);
        let mut chunk = [0u8; 4096];

        // Handshake
        let key = loop {
            match parse_handshake(&buf, transport) {
                Ok(key) => break key,
                Err(HandshakeError::Incomplete) => {}
                Err(err) => {
                    let _ = stream.write_all(error_response(&err).as_bytes()).await;
                    open.fetch_sub(1, Ordering::Relaxed);
                    return;
                }
            }
            match stream.read(&mut chunk).await {
                Ok(0) | Err(_) => {
                    open.fetch_sub(1, Ordering::Relaxed);
                    return;
                }
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
        };
        if stream
            .write_all(handshake_response(&key).as_bytes())
            .await
            .is_err()
        {
            open.fetch_sub(1, Ordering::Relaxed);
            return;
        }
        // Anything after the request's blank line is already frame data.
        let end = buf
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .map_or(buf.len(), |p| p + 4);
        buf.drain(..end);

        let mut assembler = MessageAssembler::default();
        'connection: loop {
            loop {
                match decode_frame(&buf) {
                    Ok(None) => break,
                    Err(code) => {
                        let _ = stream.write_all(&close_frame(code)).await;
                        break 'connection;
                    }
                    Ok(Some((frame, used))) => {
                        buf.drain(..used);
                        match assembler.push(frame) {
                            Step::Continue => {}
                            Step::Reply(bytes) => {
                                if stream.write_all(&bytes).await.is_err() {
                                    break 'connection;
                                }
                            }
                            Step::Close(code) => {
                                let _ = stream.write_all(&close_frame(code)).await;
                                break 'connection;
                            }
                            Step::Message(text) => match serde_json::from_str::<Value>(&text) {
                                Ok(message) if message.is_object() => {
                                    // A failed emit means the window is gone;
                                    // keep serving for the next one.
                                    let _ = app.emit(
                                        MESSAGE_EVENT,
                                        json!({ "connection": connection, "message": message }),
                                    );
                                }
                                _ => {
                                    let _ = stream.write_all(&close_frame(CLOSE_INVALID)).await;
                                    break 'connection;
                                }
                            },
                        }
                    }
                }
            }
            match stream.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
            // A peer trickling an endless partial frame is bounded by the
            // frame length check; this bounds the raw buffer too.
            if buf.len() > MAX_MESSAGE + 16 {
                let _ = stream.write_all(&close_frame(CLOSE_TOO_BIG)).await;
                break;
            }
        }
        open.fetch_sub(1, Ordering::Relaxed);
    }

    /// True when a connection may be served; counts it if so.
    fn admit(open: &AtomicUsize) -> bool {
        if open.fetch_add(1, Ordering::Relaxed) >= MAX_CONNECTIONS {
            open.fetch_sub(1, Ordering::Relaxed);
            return false;
        }
        true
    }

    async fn bind_tcp() -> Option<(TcpListener, u16)> {
        for port in PORT_FIRST..=PORT_LAST {
            if let Ok(listener) = TcpListener::bind(("127.0.0.1", port)).await {
                return Some((listener, port));
            }
        }
        None
    }

    async fn accept_tcp(listener: &Option<TcpListener>) -> std::io::Result<tokio::net::TcpStream> {
        match listener {
            Some(listener) => listener.accept().await.map(|(stream, _)| stream),
            None => std::future::pending().await,
        }
    }

    // The unix socket, behind one name per platform so the accept loop has no
    // cfg inside it. Windows has no /tmp socket to offer: the listener type is
    // a placeholder that never binds and never accepts.
    #[cfg(not(windows))]
    type LocalListener = tokio::net::UnixListener;
    #[cfg(not(windows))]
    type LocalStream = tokio::net::UnixStream;
    #[cfg(windows)]
    type LocalListener = ();
    #[cfg(windows)]
    type LocalStream = tokio::net::TcpStream;

    #[cfg(not(windows))]
    async fn bind_local(opaque_id: &str) -> Option<(LocalListener, String)> {
        unix::bind(opaque_id).await
    }

    #[cfg(windows)]
    async fn bind_local(_opaque_id: &str) -> Option<(LocalListener, String)> {
        None
    }

    #[cfg(not(windows))]
    async fn accept_local(listener: &Option<LocalListener>) -> std::io::Result<LocalStream> {
        match listener {
            Some(listener) => listener.accept().await.map(|(stream, _)| stream),
            None => std::future::pending().await,
        }
    }

    #[cfg(windows)]
    async fn accept_local(_listener: &Option<LocalListener>) -> std::io::Result<LocalStream> {
        std::future::pending().await
    }

    fn cleanup_local(path: &str) {
        #[cfg(not(windows))]
        unix::cleanup(path);
        #[cfg(windows)]
        let _ = path;
    }

    #[cfg(not(windows))]
    mod unix {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        use std::path::PathBuf;

        use tokio::net::{UnixListener, UnixStream};

        /// `/tmp/.prinny_<opaque_id>/presence.sock`, with the directory made
        /// owner-only. A directory that already exists is used only if it is a
        /// real directory with no group or other permissions; one squatted by
        /// another user with looser permissions is refused, and one squatted
        /// owner-only cannot be bound into at all.
        pub async fn bind(opaque_id: &str) -> Option<(UnixListener, String)> {
            let dir = PathBuf::from(format!("/tmp/.prinny_{opaque_id}"));
            match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                    let meta = std::fs::symlink_metadata(&dir).ok()?;
                    if !meta.is_dir() || meta.permissions().mode() & 0o077 != 0 {
                        return None;
                    }
                }
                Err(_) => return None,
            }
            let path = dir.join("presence.sock");
            let path_str = path.to_string_lossy().into_owned();
            // A crashed run leaves the socket file behind and bind() will not
            // replace it; remove it only when nothing answers on it.
            if UnixStream::connect(&path).await.is_ok() {
                return None;
            }
            let _ = std::fs::remove_file(&path);
            let listener = UnixListener::bind(&path).ok()?;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
            Some((listener, path_str))
        }

        pub fn cleanup(path: &str) {
            let _ = std::fs::remove_file(path);
            if let Some(dir) = std::path::Path::new(path).parent() {
                let _ = std::fs::remove_dir(dir);
            }
        }
    }

    pub struct ServerHandle {
        pub bound: AppPresenceStarted,
        shutdown: watch::Sender<bool>,
    }

    impl ServerHandle {
        pub fn stop(&self) {
            let _ = self.shutdown.send(true);
        }
    }

    /// Binds and serves on a thread with its own runtime, like the Discord
    /// bridge, so the accept loops never occupy Tauri's shared runtime.
    pub async fn start<R: Runtime>(
        app: AppHandle<R>,
        opaque_id: String,
    ) -> Result<ServerHandle, String> {
        let (ready_tx, ready_rx) = oneshot::channel::<Result<AppPresenceStarted, String>>();
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);

        std::thread::Builder::new()
            .name("app-presence".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_io()
                    .enable_time()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(err) => {
                        let _ = ready_tx.send(Err(err.to_string()));
                        return;
                    }
                };

                runtime.block_on(async move {
                    let tcp = bind_tcp().await;
                    let local = bind_local(&opaque_id).await;
                    let socket = local.as_ref().map(|(_, path)| path.clone());

                    if tcp.is_none() && socket.is_none() {
                        let _ = ready_tx.send(Err(format!(
                            "no free port in {PORT_FIRST}-{PORT_LAST} and no unix socket"
                        )));
                        return;
                    }
                    let _ = ready_tx.send(Ok(AppPresenceStarted {
                        port: tcp.as_ref().map(|(_, port)| *port),
                        socket: socket.clone(),
                    }));

                    let open = Arc::new(AtomicUsize::new(0));
                    let tcp_listener = tcp.map(|(listener, _)| listener);
                    let local_listener = local.map(|(listener, _)| listener);

                    loop {
                        tokio::select! {
                            _ = shutdown_rx.changed() => break,
                            accepted = accept_tcp(&tcp_listener) => {
                                let Ok(stream) = accepted else { break };
                                if admit(&open) {
                                    tokio::spawn(serve(stream, Transport::Tcp, app.clone(), Arc::clone(&open)));
                                }
                            }
                            accepted = accept_local(&local_listener) => {
                                let Ok(stream) = accepted else { break };
                                if admit(&open) {
                                    tokio::spawn(serve(stream, Transport::Unix, app.clone(), Arc::clone(&open)));
                                }
                            }
                        }
                    }

                    if let Some(path) = socket {
                        cleanup_local(&path);
                    }
                });
            })
            .map_err(|err| format!("could not start app presence thread: {err}"))?;

        let bound = ready_rx
            .await
            .map_err(|_| "app presence thread exited before binding".to_owned())??;

        Ok(ServerHandle {
            bound,
            shutdown: shutdown_tx,
        })
    }
}

/// Managed state: the running server, if any.
#[derive(Default)]
pub struct AppPresenceServer {
    #[cfg(not(mobile))]
    handle: Mutex<Option<imp::ServerHandle>>,
    #[cfg(mobile)]
    _unused: Mutex<()>,
}

#[cfg(not(mobile))]
#[tauri::command]
pub async fn start_app_presence_server<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: tauri::State<'_, AppPresenceServer>,
    opaque_id: String,
) -> Result<AppPresenceStarted, String> {
    if !valid_opaque_id(&opaque_id) {
        return Err("opaque id must be 16-64 lowercase hex characters".to_owned());
    }
    {
        let guard = state
            .handle
            .lock()
            .map_err(|_| "app presence state poisoned".to_owned())?;
        if let Some(handle) = guard.as_ref() {
            return Ok(handle.bound.clone());
        }
    }

    let handle = imp::start(app, opaque_id).await?;
    let bound = handle.bound.clone();

    let mut guard = state
        .handle
        .lock()
        .map_err(|_| "app presence state poisoned".to_owned())?;
    if let Some(existing) = guard.as_ref() {
        handle.stop();
        return Ok(existing.bound.clone());
    }
    *guard = Some(handle);
    Ok(bound)
}

#[cfg(not(mobile))]
#[tauri::command]
pub async fn stop_app_presence_server(
    state: tauri::State<'_, AppPresenceServer>,
) -> Result<(), String> {
    let handle = {
        let mut guard = state
            .handle
            .lock()
            .map_err(|_| "app presence state poisoned".to_owned())?;
        guard.take()
    };
    if let Some(handle) = handle {
        handle.stop();
    }
    Ok(())
}

// Mobile has no local apps to hear from; the commands exist so the one
// `generate_handler!` list compiles for every target.
#[cfg(mobile)]
#[tauri::command]
pub async fn start_app_presence_server(
    _state: tauri::State<'_, AppPresenceServer>,
    _opaque_id: String,
) -> Result<AppPresenceStarted, String> {
    Err("the app presence server is desktop only".to_owned())
}

#[cfg(mobile)]
#[tauri::command]
pub async fn stop_app_presence_server(
    _state: tauri::State<'_, AppPresenceServer>,
) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn sha1_matches_fips_vectors() {
        assert_eq!(
            hex(&sha1(b"abc")),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(hex(&sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(
            hex(&sha1(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
    }

    #[test]
    fn base64_pads_every_remainder() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn accept_key_matches_rfc_6455() {
        assert_eq!(
            accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }

    fn request(extra: &str) -> Vec<u8> {
        format!(
            "GET /presence HTTP/1.1\r\nHost: localhost:44190\r\nUpgrade: websocket\r\nConnection: keep-alive, Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n{extra}\r\n"
        )
        .into_bytes()
    }

    #[test]
    fn accepts_a_native_handshake() {
        assert_eq!(
            parse_handshake(&request(""), Transport::Tcp),
            Ok("dGhlIHNhbXBsZSBub25jZQ==".to_owned())
        );
    }

    #[test]
    fn refuses_browsers_and_foreign_hosts() {
        assert!(matches!(
            parse_handshake(&request("Origin: https://evil.example\r\n"), Transport::Tcp),
            Err(HandshakeError::Forbidden(_))
        ));
        let rebound = String::from_utf8(request(""))
            .unwrap()
            .replace("Host: localhost:44190", "Host: evil.example:44190");
        assert!(matches!(
            parse_handshake(rebound.as_bytes(), Transport::Tcp),
            Err(HandshakeError::Forbidden(_))
        ));
        // Over the unix socket Host carries no meaning
        assert!(parse_handshake(rebound.as_bytes(), Transport::Unix).is_ok());
    }

    #[test]
    fn loopback_hosts() {
        for host in [
            "localhost",
            "localhost:44190",
            "127.0.0.1:44191",
            "[::1]:44190",
            "[::1]",
        ] {
            assert!(is_loopback_host(host), "{host}");
        }
        for host in [
            "evil.example",
            "localhost.evil.example",
            "127.0.0.2",
            "10.0.0.1:44190",
        ] {
            assert!(!is_loopback_host(host), "{host}");
        }
    }

    #[test]
    fn handshake_waits_for_the_whole_request_and_bounds_it() {
        let full = request("");
        assert_eq!(
            parse_handshake(&full[..full.len() - 3], Transport::Tcp),
            Err(HandshakeError::Incomplete)
        );
        assert_eq!(
            parse_handshake(&vec![b'a'; MAX_HANDSHAKE + 1], Transport::Tcp),
            Err(HandshakeError::TooLarge)
        );
        let wrong_path = String::from_utf8(full)
            .unwrap()
            .replace("/presence", "/other");
        assert_eq!(
            parse_handshake(wrong_path.as_bytes(), Transport::Tcp),
            Err(HandshakeError::NotFound)
        );
    }

    fn masked(opcode: u8, fin: bool, payload: &[u8]) -> Vec<u8> {
        let mask = [0x37, 0xfa, 0x21, 0x3d];
        let mut out = vec![if fin { 0x80 } else { 0 } | opcode];
        match payload.len() {
            len @ 0..=125 => out.push(0x80 | len as u8),
            len => {
                out.push(0x80 | 126);
                out.extend_from_slice(&(len as u16).to_be_bytes());
            }
        }
        out.extend_from_slice(&mask);
        out.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
        out
    }

    #[test]
    fn decodes_the_rfc_masked_hello() {
        // RFC 6455 §5.7: a single-frame masked text message "Hello"
        let bytes = [
            0x81, 0x85, 0x37, 0xfa, 0x21, 0x3d, 0x7f, 0x9f, 0x4d, 0x51, 0x58,
        ];
        let (frame, used) = decode_frame(&bytes).unwrap().unwrap();
        assert_eq!(used, bytes.len());
        assert_eq!(
            frame,
            Frame {
                fin: true,
                opcode: OP_TEXT,
                payload: b"Hello".to_vec()
            }
        );
        assert_eq!(decode_frame(&bytes[..6]), Ok(None));
    }

    #[test]
    fn rejects_unmasked_oversized_and_reserved_frames() {
        assert_eq!(
            decode_frame(&[0x81, 0x05, b'H', b'e', b'l', b'l', b'o']),
            Err(CLOSE_PROTOCOL)
        );
        assert_eq!(decode_frame(&[0xC1, 0x80, 0, 0, 0, 0]), Err(CLOSE_PROTOCOL));
        let mut huge = vec![0x81, 0x80 | 127];
        huge.extend_from_slice(&((MAX_MESSAGE as u64) + 1).to_be_bytes());
        assert_eq!(decode_frame(&huge), Err(CLOSE_TOO_BIG));
        // A control frame may not be fragmented
        assert_eq!(
            decode_frame(&masked(OP_PING, false, b"")),
            Err(CLOSE_PROTOCOL)
        );
    }

    #[test]
    fn reads_a_medium_length_frame() {
        let payload = vec![b'x'; 300];
        let (frame, _) = decode_frame(&masked(OP_TEXT, true, &payload))
            .unwrap()
            .unwrap();
        assert_eq!(frame.payload, payload);
    }

    #[test]
    fn reassembles_fragments_and_answers_pings_between_them() {
        let mut assembler = MessageAssembler::default();
        let decode = |bytes: Vec<u8>| decode_frame(&bytes).unwrap().unwrap().0;
        assert_eq!(
            assembler.push(decode(masked(OP_TEXT, false, b"{\"a\":"))),
            Step::Continue
        );
        assert_eq!(
            assembler.push(decode(masked(OP_PING, true, b"p"))),
            Step::Reply(encode_frame(OP_PONG, b"p"))
        );
        assert_eq!(
            assembler.push(decode(masked(OP_CONTINUATION, true, b"1}"))),
            Step::Message("{\"a\":1}".to_owned())
        );
        assert_eq!(
            assembler.push(decode(masked(OP_CONTINUATION, true, b"x"))),
            Step::Close(CLOSE_PROTOCOL)
        );
    }

    #[test]
    fn closes_on_binary_and_invalid_utf8() {
        let mut assembler = MessageAssembler::default();
        assert_eq!(
            assembler.push(Frame {
                fin: true,
                opcode: OP_BINARY,
                payload: vec![1]
            }),
            Step::Close(CLOSE_UNSUPPORTED)
        );
        assert_eq!(
            MessageAssembler::default().push(Frame {
                fin: true,
                opcode: OP_TEXT,
                payload: vec![0xff]
            }),
            Step::Close(CLOSE_INVALID)
        );
    }

    #[test]
    fn encodes_server_frames_unmasked() {
        assert_eq!(encode_frame(OP_TEXT, b"Hi"), vec![0x81, 0x02, b'H', b'i']);
        let long = encode_frame(OP_TEXT, &[0u8; 200]);
        assert_eq!(&long[..4], &[0x81, 126, 0, 200]);
        assert_eq!(close_frame(1000), vec![0x88, 0x02, 0x03, 0xe8]);
    }

    #[test]
    fn opaque_ids_cannot_steer_the_path() {
        assert!(valid_opaque_id("0123456789abcdef"));
        assert!(!valid_opaque_id("0123456789ABCDEF"));
        assert!(!valid_opaque_id("../../etc/passwd"));
        assert!(!valid_opaque_id("abc"));
    }
}
