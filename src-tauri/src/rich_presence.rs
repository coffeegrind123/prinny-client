// Impersonates Discord's local RPC server so any Discord-RPC-aware client
// (media players, MPRIS bridges, games, editors, ...) publishes its activity
// into Matrix as MSC4320 rich presence. Desktop only, and only while the
// frontend's `publishRichPresence` setting is on: nothing here starts by
// itself.
//
// The wire protocol mirrors arRPC's implementation
// (https://github.com/OpenAsar/arRPC): framed `[i32 LE op][i32 LE len][json]`,
// with HANDSHAKE -> READY, SET_ACTIVITY -> activity, PING -> PONG.
//
// We bind the first free discord-ipc-{n} slot (n = 0..9), probing by
// *connecting* rather than by trying to listen, so a Discord instance already
// owning a lower slot is never clobbered: we only receive clients while we hold
// the lowest slot, i.e. when Discord itself is not running. Mutually exclusive
// in practice, non-destructive always.
//
// The activity object is forwarded to the webview verbatim. Rust does not
// interpret it — the mapping to MSC4320 lives in `src/app/utils/discordActivity.ts`
// in the webapp, which is where it can be unit-tested and changed without a
// native rebuild.

use std::sync::Mutex;

use serde::Serialize;

/// Event carrying the current activity, or `null` when nothing is playing.
/// Only the desktop implementation emits it; on mobile there is nothing to emit.
#[cfg(not(mobile))]
pub const ACTIVITY_EVENT: &str = "rich-presence-activity";

#[derive(Serialize, Clone, Debug)]
pub struct BridgeStarted {
    /// The pipe or socket path we bound, shown in settings.
    pub path: String,
    /// Which discord-ipc slot it is. Anything above 0 means another RPC server
    /// holds a lower one and will be handed activity before us.
    pub index: u32,
}

#[cfg(not(mobile))]
mod imp {
    use super::{BridgeStarted, ACTIVITY_EVENT};

    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    use serde_json::{json, Value};
    use tauri::{AppHandle, Emitter, Runtime};
    use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
    use tokio::sync::{oneshot, watch};

    const OP_HANDSHAKE: i32 = 0;
    const OP_FRAME: i32 = 1;
    const OP_CLOSE: i32 = 2;
    const OP_PING: i32 = 3;
    const OP_PONG: i32 = 4;

    const CLOSE_NORMAL: i32 = 1000;
    const CLOSE_UNSUPPORTED: i32 = 1003;
    const ERR_INVALID_CLIENTID: i32 = 4000;
    const ERR_INVALID_VERSION: i32 = 4004;

    // A frame larger than this is a malformed or hostile client, not a real
    // activity: the largest legitimate SET_ACTIVITY is a few hundred bytes.
    const MAX_FRAME: usize = 1024 * 1024;
    const MAX_SLOTS: u32 = 10;

    static NEXT_CONNECTION_ID: AtomicU64 = AtomicU64::new(1);

    /// Per-connection last activity, in most-recently-updated order. The
    /// current one is the newest non-null entry, which is last-write-wins and
    /// matches what Discord itself does with several connected clients.
    #[derive(Default)]
    struct Activities {
        order: Vec<u64>,
        by_connection: HashMap<u64, Option<Value>>,
    }

    impl Activities {
        fn current(&self) -> Option<Value> {
            self.order
                .iter()
                .rev()
                .find_map(|id| self.by_connection.get(id).cloned().flatten())
        }
    }

    fn emit_current<R: Runtime>(app: &AppHandle<R>, activities: &Mutex<Activities>) {
        let current = match activities.lock() {
            Ok(guard) => guard.current(),
            Err(poisoned) => poisoned.into_inner().current(),
        };
        // A failed emit means the window is gone; there is nothing useful to do
        // about it here and the bridge should keep running for the next one.
        let _ = app.emit(ACTIVITY_EVENT, current);
    }

    fn encode(op: i32, payload: &Value) -> Vec<u8> {
        // Serializing a Value cannot fail in practice; an empty object is a
        // harmless frame if it somehow does.
        let body = serde_json::to_vec(payload).unwrap_or_else(|_| b"{}".to_vec());
        let mut out = Vec::with_capacity(8 + body.len());
        out.extend_from_slice(&op.to_le_bytes());
        out.extend_from_slice(&(body.len() as i32).to_le_bytes());
        out.extend_from_slice(&body);
        out
    }

    /// Inert mock identity returned in the READY dispatch. Clients only need a
    /// plausible user object to proceed; nothing is ever sent to Discord.
    fn ready_frame() -> Value {
        json!({
            "cmd": "DISPATCH",
            "data": {
                "v": 1,
                "config": {
                    "cdn_host": "cdn.discordapp.com",
                    "api_endpoint": "//discord.com/api",
                    "environment": "production",
                },
                "user": {
                    "id": "1045800378228281345",
                    "username": "prinny",
                    "discriminator": "0",
                    "global_name": "Prinny",
                    "avatar": null,
                    "avatar_decoration_data": null,
                    "bot": false,
                    "flags": 0,
                    "premium_type": 0,
                },
            },
            "evt": "READY",
            "nonce": null,
        })
    }

    /// Reads one complete frame, buffering across partial reads. `Ok(None)`
    /// means the peer closed cleanly.
    async fn read_frame<S>(
        stream: &mut S,
        buf: &mut Vec<u8>,
    ) -> Result<Option<(i32, Value)>, String>
    where
        S: AsyncRead + Unpin,
    {
        loop {
            if buf.len() >= 8 {
                let op = i32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
                let len = i32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
                if !(OP_HANDSHAKE..=OP_PONG).contains(&op) || len < 0 {
                    return Err(format!("invalid frame (op={op}, len={len})"));
                }
                let len = len as usize;
                if len > MAX_FRAME {
                    return Err(format!("frame too large ({len} bytes)"));
                }
                let total = 8 + len;
                if buf.len() >= total {
                    let value: Value = if len == 0 {
                        Value::Null
                    } else {
                        serde_json::from_slice(&buf[8..total])
                            .map_err(|err| format!("invalid frame body: {err}"))?
                    };
                    buf.drain(..total);
                    return Ok(Some((op, value)));
                }
            }

            let mut chunk = [0u8; 8192];
            let read = stream
                .read(&mut chunk)
                .await
                .map_err(|err| err.to_string())?;
            if read == 0 {
                return Ok(None);
            }
            buf.extend_from_slice(&chunk[..read]);
        }
    }

    async fn serve_connection<S, R>(
        mut stream: S,
        connection_id: u64,
        app: AppHandle<R>,
        activities: Arc<Mutex<Activities>>,
    ) where
        S: AsyncRead + AsyncWrite + Unpin,
        R: Runtime,
    {
        {
            let mut guard = match activities.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            guard.order.push(connection_id);
            guard.by_connection.insert(connection_id, None);
        }

        let mut buf: Vec<u8> = Vec::new();
        let mut client_id = String::new();
        let mut handshaken = false;

        loop {
            match read_frame(&mut stream, &mut buf).await {
                Ok(None) | Err(_) => break,
                Ok(Some((op, payload))) => {
                    match op {
                        OP_PING => {
                            if stream.write_all(&encode(OP_PONG, &payload)).await.is_err() {
                                break;
                            }
                        }
                        OP_PONG => {}
                        OP_CLOSE => break,
                        OP_HANDSHAKE => {
                            // `v` arrives as a number from most clients and a
                            // string from a few, so accept either rather than
                            // rejecting a client over its JSON typing.
                            let version = payload
                                .get("v")
                                .and_then(|v| {
                                    v.as_i64()
                                        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                                })
                                .unwrap_or(1);
                            let id = payload
                                .get("client_id")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default();

                            if version != 1 {
                                let _ = stream
                                    .write_all(&encode(
                                        OP_CLOSE,
                                        &json!({
                                            "code": ERR_INVALID_VERSION,
                                            "message": "unsupported version",
                                        }),
                                    ))
                                    .await;
                                break;
                            }
                            if id.is_empty() {
                                let _ = stream
                                    .write_all(&encode(
                                        OP_CLOSE,
                                        &json!({
                                            "code": ERR_INVALID_CLIENTID,
                                            "message": "client id required",
                                        }),
                                    ))
                                    .await;
                                break;
                            }

                            client_id = id.to_owned();
                            handshaken = true;
                            if stream
                                .write_all(&encode(OP_FRAME, &ready_frame()))
                                .await
                                .is_err()
                            {
                                break;
                            }
                        }
                        OP_FRAME => {
                            if !handshaken {
                                let _ = stream
                                    .write_all(&encode(
                                        OP_CLOSE,
                                        &json!({
                                            "code": CLOSE_UNSUPPORTED,
                                            "message": "handshake required",
                                        }),
                                    ))
                                    .await;
                                break;
                            }

                            let cmd = payload.get("cmd").and_then(|v| v.as_str()).unwrap_or("");
                            let nonce = payload.get("nonce").cloned().unwrap_or(Value::Null);

                            if cmd == "SET_ACTIVITY" {
                                let activity = payload
                                    .get("args")
                                    .and_then(|args| args.get("activity"))
                                    .filter(|activity| !activity.is_null())
                                    .cloned();

                                {
                                    let mut guard = match activities.lock() {
                                        Ok(guard) => guard,
                                        Err(poisoned) => poisoned.into_inner(),
                                    };
                                    guard.by_connection.insert(connection_id, activity.clone());
                                    guard.order.retain(|id| *id != connection_id);
                                    guard.order.push(connection_id);
                                }
                                emit_current(&app, &activities);

                                let echo = match activity {
                                    Some(mut activity) => {
                                        if let Some(obj) = activity.as_object_mut() {
                                            obj.insert("name".into(), Value::String(String::new()));
                                            obj.insert(
                                                "application_id".into(),
                                                Value::String(client_id.clone()),
                                            );
                                            obj.entry("type").or_insert(json!(0));
                                        }
                                        activity
                                    }
                                    None => Value::Null,
                                };
                                if stream
                                    .write_all(&encode(
                                        OP_FRAME,
                                        &json!({
                                            "cmd": cmd,
                                            "data": echo,
                                            "evt": null,
                                            "nonce": nonce,
                                        }),
                                    ))
                                    .await
                                    .is_err()
                                {
                                    break;
                                }
                                continue;
                            }

                            // SUBSCRIBE / UNSUBSCRIBE / AUTHENTICATE and the
                            // rest: acknowledge and ignore, so a client does
                            // not hang waiting on a reply it does not need.
                            if stream
                                .write_all(&encode(
                                    OP_FRAME,
                                    &json!({
                                        "cmd": cmd,
                                        "data": null,
                                        "evt": null,
                                        "nonce": nonce,
                                    }),
                                ))
                                .await
                                .is_err()
                            {
                                break;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        let _ = stream
            .write_all(&encode(
                OP_CLOSE,
                &json!({ "code": CLOSE_NORMAL, "message": "" }),
            ))
            .await;
        let _ = stream.shutdown().await;

        {
            let mut guard = match activities.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            guard.order.retain(|id| *id != connection_id);
            guard.by_connection.remove(&connection_id);
        }
        emit_current(&app, &activities);
    }

    #[cfg(windows)]
    mod platform {
        use tokio::net::windows::named_pipe::ClientOptions;

        // A pipe that exists but has every instance busy still proves a server
        // is there, which is the only thing the probe is asking.
        const ERROR_PIPE_BUSY: i32 = 231;

        pub fn socket_path(slot: u32) -> Option<String> {
            Some(format!(r"\\?\pipe\discord-ipc-{slot}"))
        }

        pub async fn slot_is_free(path: &str) -> bool {
            match ClientOptions::new().open(path) {
                Ok(_) => false,
                Err(err) if err.raw_os_error() == Some(ERROR_PIPE_BUSY) => false,
                Err(_) => true,
            }
        }

        /// Nothing to clean up: a named pipe disappears with its last handle.
        pub fn remove_stale(_path: &str) {}
    }

    #[cfg(not(windows))]
    mod platform {
        use std::path::PathBuf;

        use tokio::net::UnixStream;

        /// The directory the IPC socket lives in.
        ///
        /// XDG_RUNTIME_DIR only. The chain used to fall through TMPDIR, TMP and
        /// TEMP to a hard-coded `/tmp`, which is world-traversable: on a host
        /// where none of those was set, any OTHER local user could connect and
        /// publish arbitrary text as this user's Matrix rich presence, and feed
        /// arbitrary JSON to the webview. Accepting activity from same-user
        /// processes is the intended Discord-RPC contract; accepting it from a
        /// different user is not.
        ///
        /// Returns None when no per-user runtime directory can be established,
        /// in which case the bridge simply does not start - the feature is
        /// optional, and silently downgrading its trust boundary is not.
        pub fn socket_dir() -> Option<PathBuf> {
            let dir = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?);
            if !dir_is_private(&dir) {
                return None;
            }
            Some(dir)
        }

        /// True when the directory denies group and other entirely.
        ///
        /// XDG_RUNTIME_DIR is specified as a per-user directory created 0700, so
        /// the mode is the load-bearing property; a directory that has been
        /// loosened is refused rather than trusted for its name.
        fn dir_is_private(dir: &std::path::Path) -> bool {
            use std::os::unix::fs::PermissionsExt;
            let Ok(meta) = std::fs::metadata(dir) else {
                return false;
            };
            meta.is_dir() && meta.permissions().mode() & 0o077 == 0
        }

        pub fn socket_path(slot: u32) -> Option<String> {
            Some(
                socket_dir()?
                    .join(format!("discord-ipc-{slot}"))
                    .to_string_lossy()
                    .into_owned(),
            )
        }

        pub async fn slot_is_free(path: &str) -> bool {
            // Connect refused or no such file: no live server, so the slot is
            // ours. A successful connect means someone is serving it.
            UnixStream::connect(path).await.is_err()
        }

        /// A crashed server leaves its socket file behind, and bind() will not
        /// replace it. Removing it is safe precisely because the probe above
        /// just established that nothing is listening on it.
        pub fn remove_stale(path: &str) {
            let _ = std::fs::remove_file(path);
        }
    }

    async fn find_free_slot() -> Option<BridgeStarted> {
        for slot in 0..MAX_SLOTS {
            // None means there is no directory private to this user to bind in,
            // so the bridge does not start at all.
            let path = platform::socket_path(slot)?;
            if platform::slot_is_free(&path).await {
                platform::remove_stale(&path);
                return Some(BridgeStarted { path, index: slot });
            }
        }
        None
    }

    #[cfg(windows)]
    async fn accept_loop<R: Runtime>(
        path: String,
        app: AppHandle<R>,
        activities: Arc<Mutex<Activities>>,
        mut shutdown: watch::Receiver<bool>,
    ) {
        use tokio::net::windows::named_pipe::ServerOptions;

        // `first_pipe_instance` fails rather than joining a pipe another
        // process already owns: a second guard behind the probe, and the one
        // that closes the race between probing and binding.
        let mut server = match ServerOptions::new().first_pipe_instance(true).create(&path) {
            Ok(server) => server,
            Err(_) => return,
        };

        loop {
            tokio::select! {
                _ = shutdown.changed() => break,
                connected = server.connect() => {
                    if connected.is_err() {
                        break;
                    }
                    let next = match ServerOptions::new().create(&path) {
                        Ok(next) => next,
                        Err(_) => break,
                    };
                    let stream = std::mem::replace(&mut server, next);
                    let id = NEXT_CONNECTION_ID.fetch_add(1, Ordering::Relaxed);
                    tokio::spawn(serve_connection(
                        stream,
                        id,
                        app.clone(),
                        Arc::clone(&activities),
                    ));
                }
            }
        }
    }

    #[cfg(not(windows))]
    async fn accept_loop<R: Runtime>(
        path: String,
        app: AppHandle<R>,
        activities: Arc<Mutex<Activities>>,
        mut shutdown: watch::Receiver<bool>,
    ) {
        use tokio::net::UnixListener;

        let listener = match UnixListener::bind(&path) {
            Ok(listener) => listener,
            Err(_) => return,
        };

        // Owner-only, regardless of umask. The directory check already keeps
        // other users out, but a socket left group- or world-connectable is one
        // umask away from undoing it.
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }

        loop {
            tokio::select! {
                _ = shutdown.changed() => break,
                accepted = listener.accept() => {
                    let stream = match accepted {
                        Ok((stream, _)) => stream,
                        Err(_) => break,
                    };
                    let id = NEXT_CONNECTION_ID.fetch_add(1, Ordering::Relaxed);
                    tokio::spawn(serve_connection(
                        stream,
                        id,
                        app.clone(),
                        Arc::clone(&activities),
                    ));
                }
            }
        }

        drop(listener);
        platform::remove_stale(&path);
    }

    /// Owns the server thread. Dropping the sender is not enough to stop it —
    /// the shutdown value is sent explicitly — so the handle is kept in managed
    /// state until `stop_rich_presence_bridge` or app exit.
    pub struct BridgeHandle {
        pub bound: BridgeStarted,
        shutdown: watch::Sender<bool>,
    }

    impl BridgeHandle {
        pub fn stop(&self) {
            let _ = self.shutdown.send(true);
        }
    }

    /// Binds a slot and starts serving on a thread of its own.
    ///
    /// The server runs on a runtime this function builds rather than on Tauri's
    /// shared one. A long-lived accept loop has no business occupying a slot in
    /// the runtime the rest of the app's commands share, and building it here
    /// also means the IO driver this needs is enabled by construction instead
    /// of by assumption about how the host runtime was configured.
    pub async fn start<R: Runtime>(app: AppHandle<R>) -> Result<BridgeHandle, String> {
        let (ready_tx, ready_rx) = oneshot::channel::<Result<BridgeStarted, String>>();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        std::thread::Builder::new()
            .name("rich-presence".into())
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
                    let bound = match find_free_slot().await {
                        Some(bound) => bound,
                        None => {
                            let _ = ready_tx
                                .send(Err("all discord-ipc slots (0-9) are in use".to_owned()));
                            return;
                        }
                    };

                    let activities = Arc::new(Mutex::new(Activities::default()));
                    let path = bound.path.clone();
                    let _ = ready_tx.send(Ok(bound));

                    accept_loop(path, app.clone(), Arc::clone(&activities), shutdown_rx).await;

                    // Whatever was playing is no longer ours to report.
                    let _ = app.emit(ACTIVITY_EVENT, Option::<Value>::None);
                });
            })
            .map_err(|err| format!("could not start bridge thread: {err}"))?;

        let bound = ready_rx
            .await
            .map_err(|_| "bridge thread exited before binding".to_owned())??;

        Ok(BridgeHandle {
            bound,
            shutdown: shutdown_tx,
        })
    }
}

/// Managed state: the running bridge, if any.
#[derive(Default)]
pub struct RichPresenceBridge {
    #[cfg(not(mobile))]
    handle: Mutex<Option<imp::BridgeHandle>>,
    #[cfg(mobile)]
    _unused: Mutex<()>,
}

#[cfg(not(mobile))]
#[tauri::command]
pub async fn start_rich_presence_bridge<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: tauri::State<'_, RichPresenceBridge>,
) -> Result<BridgeStarted, String> {
    // Already running: report the slot we hold rather than binding a second
    // one. The frontend re-invokes this whenever the setting is toggled or the
    // client remounts.
    {
        let guard = state
            .handle
            .lock()
            .map_err(|_| "rich presence state poisoned".to_owned())?;
        if let Some(handle) = guard.as_ref() {
            return Ok(handle.bound.clone());
        }
    }

    let handle = imp::start(app).await?;
    let bound = handle.bound.clone();

    let mut guard = state
        .handle
        .lock()
        .map_err(|_| "rich presence state poisoned".to_owned())?;
    if let Some(existing) = guard.as_ref() {
        // Two starts raced. Keep the one already stored and retire ours, so the
        // slot count cannot creep upward.
        handle.stop();
        return Ok(existing.bound.clone());
    }
    *guard = Some(handle);

    Ok(bound)
}

#[cfg(not(mobile))]
#[tauri::command]
pub async fn stop_rich_presence_bridge<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: tauri::State<'_, RichPresenceBridge>,
) -> Result<(), String> {
    let handle = {
        let mut guard = state
            .handle
            .lock()
            .map_err(|_| "rich presence state poisoned".to_owned())?;
        guard.take()
    };

    if let Some(handle) = handle {
        handle.stop();
    }

    use tauri::Emitter;
    let _ = app.emit(ACTIVITY_EVENT, Option::<serde_json::Value>::None);

    Ok(())
}

// Mobile has no Discord RPC pipe to impersonate and no desktop clients to hear
// from. The commands still exist so the one `generate_handler!` list compiles
// for every target; the frontend gates on `isTauriDesktop()` and never calls
// them here.
#[cfg(mobile)]
#[tauri::command]
pub async fn start_rich_presence_bridge<R: tauri::Runtime>(
    _app: tauri::AppHandle<R>,
    _state: tauri::State<'_, RichPresenceBridge>,
) -> Result<BridgeStarted, String> {
    Err("the rich presence bridge is desktop only".to_owned())
}

#[cfg(mobile)]
#[tauri::command]
pub async fn stop_rich_presence_bridge<R: tauri::Runtime>(
    _app: tauri::AppHandle<R>,
    _state: tauri::State<'_, RichPresenceBridge>,
) -> Result<(), String> {
    Ok(())
}
