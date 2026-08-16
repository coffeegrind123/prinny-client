#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

// mod menu;

use std::collections::HashSet;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::Mutex;

use tauri::{webview::{NewWindowResponse, WebviewWindowBuilder}, Manager, WebviewUrl};
#[cfg(target_os = "macos")]
use tauri::TitleBarStyle;
use tauri_plugin_opener::OpenerExt;

mod taskbar;
mod rich_presence;

// Paths the user actually dropped onto the window via the OS native drag-drop
// path. `read_dropped_file` only reads paths that appear here, so a malicious
// in-page script can't invoke it with an arbitrary path (e.g. /etc/passwd).
#[derive(Default)]
struct DroppedPaths(Mutex<HashSet<PathBuf>>);

// The homeserver origin the application is actually connected to, recorded once
// by the frontend rather than passed per-call. `cache_notification_icon` uses it
// to decide whether the private-address guard may be relaxed; see that command
// for why a per-call argument could not be trusted for that decision.
#[derive(Default)]
struct HomeserverOrigin(Mutex<Option<String>>);

// When our own UI last asked to capture the microphone or camera.
//
// WebKitGTK's `permission-request` signal fires for every frame in the webview
// and — unlike Android's `WebChromeClient.onPermissionRequest`, which hands us
// `request.origin` — the 2.0 bindings expose no way to learn which frame asked.
// Granting unconditionally would therefore hand the mic to any iframe the user
// happens to load (a link preview, a call widget, later an integration-manager
// widget), silently and with no indication.
//
// So the frontend arms this immediately before it calls getUserMedia itself,
// and the handler only allows a request that arrives inside the window below.
// Anything else is denied. A page script can call the command too, but it
// cannot use the grant without also being the thing that calls getUserMedia
// next — and page script in OUR origin is already the trusted party here.
#[cfg(all(not(mobile), target_os = "linux"))]
#[derive(Default)]
struct CaptureIntent(Mutex<Option<std::time::Instant>>);

#[cfg(all(not(mobile), target_os = "linux"))]
const CAPTURE_INTENT_WINDOW: std::time::Duration = std::time::Duration::from_secs(15);

// True while a call is running.
//
// The one-shot intent above works for capture our own UI initiates — a voice
// message, the settings probe — because we call getUserMedia ourselves
// immediately afterwards. It does NOT work for a call: Element Call runs in an
// iframe and calls getUserMedia/getDisplayMedia itself, at whatever moment the
// user presses a button inside it. Nothing on our side is in a position to arm
// a 15-second window at that instant, so screen sharing from within a call was
// denied by the handler with no way for the user to tell why.
//
// So a call holds the gate open for its duration instead. This is a real
// widening: while a call is up, any frame in the webview could obtain capture.
// It is bounded by the call actually running, and by the frontend clearing it
// on leave — see useCallCaptureSession.
#[cfg(all(not(mobile), target_os = "linux"))]
#[derive(Default)]
struct CaptureSession(Mutex<bool>);

// Arms the capture window. Called by the frontend right before getUserMedia.
#[cfg(all(not(mobile), target_os = "linux"))]
#[tauri::command]
fn arm_capture_intent(state: tauri::State<'_, CaptureIntent>) -> Result<(), String> {
    let mut guard = state.0.lock().map_err(|_| "state poisoned".to_string())?;
    *guard = Some(std::time::Instant::now());
    Ok(())
}

// Opens or closes the call-duration capture gate.
#[cfg(all(not(mobile), target_os = "linux"))]
#[tauri::command]
fn set_capture_session(state: tauri::State<'_, CaptureSession>, active: bool) -> Result<(), String> {
    let mut guard = state.0.lock().map_err(|_| "state poisoned".to_string())?;
    *guard = active;
    Ok(())
}

#[cfg(not(all(not(mobile), target_os = "linux")))]
#[tauri::command]
fn set_capture_session(active: bool) -> Result<(), String> {
    let _ = active;
    Ok(())
}

// Non-Linux shells do their own gating: Android checks the frame origin in
// MainActivity.onPermissionRequest, and WebView2/WKWebView prompt the user.
// The command still exists everywhere so the frontend needs no per-platform
// branch at the call site.
#[cfg(not(all(not(mobile), target_os = "linux")))]
#[tauri::command]
fn arm_capture_intent() -> Result<(), String> {
    Ok(())
}

// Records the homeserver origin for the lifetime of the process. Called by the
// frontend once the Matrix client is up. Only the origin is kept — any path,
// query or fragment is discarded.
#[tauri::command]
fn set_homeserver_origin(
    state: tauri::State<'_, HomeserverOrigin>,
    origin: String,
) -> Result<(), String> {
    let parsed = reqwest::Url::parse(&origin).map_err(|e| format!("invalid origin: {e}"))?;
    match parsed.scheme() {
        "http" | "https" => {}
        other => return Err(format!("scheme not allowed: {other}")),
    }
    let normalized = format!(
        "{}://{}",
        parsed.scheme(),
        parsed
            .host_str()
            .ok_or_else(|| "origin has no host".to_string())?
    );
    let normalized = match parsed.port() {
        Some(p) => format!("{normalized}:{p}"),
        None => normalized,
    };
    let mut guard = state.0.lock().map_err(|_| "state poisoned".to_string())?;
    *guard = Some(normalized);
    Ok(())
}

// ---- SSRF / remote-fetch guards -------------------------------------------

// Media hosts our frontend legitimately proxies through `fetch_remote_bytes`
// (Twitter/X CDN via vxtwitter, Bluesky video/image CDN). Suffix-matched, so
// every subdomain (video.twimg.com, pbs.twimg.com, video.bsky.app,
// cdn.bsky.app, …) is covered. Keep this list tight — it is the allowlist that
// stops the command being used as a generic SSRF primitive.
const ALLOWED_MEDIA_HOSTS: &[&str] = &["twimg.com", "bsky.app"];

// Upper bound on any single media or notification-icon fetch. These commands
// proxy URLs chosen by remote message content, so without a cap the sender
// decides how much memory the native process allocates and how much it writes
// to disk. 64 MiB is well above any real avatar, image or short video clip.
const MEDIA_FETCH_MAX_BYTES: usize = 64 * 1024 * 1024;

// Upper bound on a single drag-dropped file read into memory and handed to the
// page. Matrix homeservers cap uploads well below this.
const DROPPED_FILE_MAX_BYTES: usize = 512 * 1024 * 1024;

fn host_allowed_media(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    ALLOWED_MEDIA_HOSTS
        .iter()
        .any(|suffix| host == *suffix || host.ends_with(&format!(".{suffix}")))
}

// True for any address a webview-supplied URL must never be allowed to reach:
// loopback, RFC1918 private, link-local (incl. 169.254.169.254 cloud
// metadata), CGNAT, benchmarking/documentation ranges, IPv6 unique-local /
// link-local, multicast, unspecified. Blocking these after DNS resolution is
// what neutralises SSRF to internal and cloud-metadata services.
fn is_disallowed_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_multicast()
                || v4.is_unspecified()
                // 0.0.0.0/8 "this network". `is_unspecified()` only matches the
                // single address 0.0.0.0, but Linux routes the whole /8 to the
                // local host, so 0.0.0.1 was reaching loopback through the gap.
                || o[0] == 0
                // 100.64.0.0/10 CGNAT
                || (o[0] == 100 && (o[1] & 0xC0) == 0x40)
                // 192.0.0.0/24 IETF protocol assignments
                || (o[0] == 192 && o[1] == 0 && o[2] == 0)
                // 198.18.0.0/15 benchmarking
                || (o[0] == 198 && (o[1] & 0xFE) == 18)
                // documentation ranges (TEST-NET-1/2/3)
                || (o[0] == 192 && o[1] == 0 && o[2] == 2)
                || (o[0] == 198 && o[1] == 51 && o[2] == 100)
                || (o[0] == 203 && o[1] == 0 && o[2] == 113)
        }
        IpAddr::V6(v6) => {
            let seg = v6.segments();

            // Unwrap every encoding that can carry an IPv4 address inside an
            // IPv6 one and re-test it as IPv4. Previously only ::ffff:0:0/96
            // (`to_ipv4_mapped`) was unwrapped, so `::127.0.0.1`,
            // `2002:7f00:1::` and `64:ff9b::7f00:1` all named loopback or
            // RFC1918 space and passed the check.
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return is_disallowed_ip(&IpAddr::V4(mapped));
            }
            // ::a.b.c.d — deprecated IPv4-compatible form. Anything in ::/96
            // other than the unspecified and loopback addresses themselves.
            if seg[0..6].iter().all(|&s| s == 0) && !(seg[6] == 0 && seg[7] <= 1) {
                let embedded = std::net::Ipv4Addr::new(
                    (seg[6] >> 8) as u8,
                    (seg[6] & 0xFF) as u8,
                    (seg[7] >> 8) as u8,
                    (seg[7] & 0xFF) as u8,
                );
                return is_disallowed_ip(&IpAddr::V4(embedded));
            }
            // 2002::/16 — 6to4 embeds the IPv4 address in the next 32 bits.
            if seg[0] == 0x2002 {
                let embedded = std::net::Ipv4Addr::new(
                    (seg[1] >> 8) as u8,
                    (seg[1] & 0xFF) as u8,
                    (seg[2] >> 8) as u8,
                    (seg[2] & 0xFF) as u8,
                );
                return is_disallowed_ip(&IpAddr::V4(embedded));
            }
            // 64:ff9b::/96 and 64:ff9b:1::/48 — NAT64 well-known prefixes.
            if seg[0] == 0x0064 && seg[1] == 0xff9b {
                let embedded = std::net::Ipv4Addr::new(
                    (seg[6] >> 8) as u8,
                    (seg[6] & 0xFF) as u8,
                    (seg[7] >> 8) as u8,
                    (seg[7] & 0xFF) as u8,
                );
                return is_disallowed_ip(&IpAddr::V4(embedded));
            }

            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                // fc00::/7 unique local
                || (seg[0] & 0xFE00) == 0xFC00
                // fe80::/10 link local
                || (seg[0] & 0xFFC0) == 0xFE80
        }
    }
}

// Resolve a host:port and reject the lookup outright if ANY resolved address is
// private/reserved (defends against a hostname that resolves to a mix of public
// and internal IPs). Returns the vetted addresses so the caller can pin them on
// the reqwest client and defeat DNS-rebinding TOCTOU between this check and the
// actual request.
async fn resolve_public_addrs(host: &str, port: u16) -> Result<Vec<SocketAddr>, String> {
    let host_owned = host.to_string();
    let addrs: Vec<SocketAddr> = tauri::async_runtime::spawn_blocking(move || {
        (host_owned.as_str(), port)
            .to_socket_addrs()
            .map(|it| it.collect::<Vec<_>>())
    })
    .await
    .map_err(|e| format!("resolve task: {e}"))?
    .map_err(|e| format!("resolve {host}: {e}"))?;

    if addrs.is_empty() {
        return Err(format!("no addresses for {host}"));
    }
    for a in &addrs {
        if is_disallowed_ip(&a.ip()) {
            return Err(format!("blocked private/reserved address {}", a.ip()));
        }
    }
    Ok(addrs)
}


// Embedded overlay icons for Windows taskbar badge (1-9, 9+)
#[cfg(target_os = "windows")]
const BADGE_ICONS: &[&[u8]] = &[
    &[], // index 0 unused
    include_bytes!("../icons/overlay/badge-1.ico"),
    include_bytes!("../icons/overlay/badge-2.ico"),
    include_bytes!("../icons/overlay/badge-3.ico"),
    include_bytes!("../icons/overlay/badge-4.ico"),
    include_bytes!("../icons/overlay/badge-5.ico"),
    include_bytes!("../icons/overlay/badge-6.ico"),
    include_bytes!("../icons/overlay/badge-7.ico"),
    include_bytes!("../icons/overlay/badge-8.ico"),
    include_bytes!("../icons/overlay/badge-9.ico"),
    include_bytes!("../icons/overlay/badge-9plus.ico"),
];

fn unifiedpush_plugin<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R> {
    tauri::plugin::Builder::new("unifiedpush")
        .setup(|_app, api| {
            #[cfg(target_os = "android")]
            {
                let _handle = api.register_android_plugin("in.prinny.app", "UnifiedPushPlugin")?;
            }
            #[cfg(not(target_os = "android"))]
            let _ = &api;
            Ok(())
        })
        .build()
}

fn foreground_plugin<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R> {
    tauri::plugin::Builder::new("foreground")
        .setup(|_app, api| {
            #[cfg(target_os = "android")]
            {
                let _handle = api.register_android_plugin("in.prinny.app", "ForegroundServicePlugin")?;
            }
            #[cfg(not(target_os = "android"))]
            let _ = &api;
            Ok(())
        })
        .build()
}

fn message_notification_plugin<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R> {
    tauri::plugin::Builder::new("messageNotification")
        .setup(|_app, api| {
            #[cfg(target_os = "android")]
            {
                let _handle = api.register_android_plugin("in.prinny.app", "MessageNotificationPlugin")?;
            }
            #[cfg(not(target_os = "android"))]
            let _ = &api;
            Ok(())
        })
        .build()
}

// Android share-sheet target. The intent filters live in AndroidManifest.xml;
// this is only the registration that makes the Kotlin plugin's commands
// (`js_ready`, `read_shared_file`) reachable from the frontend.
fn share_target_plugin<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R> {
    tauri::plugin::Builder::new("shareTarget")
        .setup(|_app, api| {
            #[cfg(target_os = "android")]
            {
                let _handle = api.register_android_plugin("in.prinny.app", "ShareTargetPlugin")?;
            }
            #[cfg(not(target_os = "android"))]
            let _ = &api;
            Ok(())
        })
        .build()
}

// Downloads a remote image (typically a Matrix sender/room avatar) and writes
// it to the OS app-cache directory. Returns the absolute path. Used by the
// notification frontend so platform code (notify-rust on desktop, our custom
// Kotlin plugin on Android) can pass a real file path to the toast — both
// notify-rust (Windows winrt-notification path) and Android's
// Notification.Builder.setLargeIcon require an actual file, not a data URI.
//
// The filename is a SHA-256 of the URL so repeat lookups hit the cache.
#[tauri::command]
async fn cache_notification_icon(
    app: tauri::AppHandle,
    url: String,
    auth_header: Option<String>,
    homeserver: Option<String>,
) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    use std::fs;

    let mut hasher = Sha256::new();
    hasher.update(url.as_bytes());
    let hash = hex::encode(&hasher.finalize()[..16]);

    let cache_dir = app
        .path()
        .app_cache_dir()
        .map_err(|e| format!("app_cache_dir: {e}"))?;
    let icons_dir = cache_dir.join("notif-icons");
    fs::create_dir_all(&icons_dir).map_err(|e| format!("create_dir_all: {e}"))?;

    // Hit cache by checking for any file that matches the hash with a real
    // image extension. Old `.img` entries (without a recognized extension)
    // are deliberately skipped so they get re-fetched with a proper ext —
    // Windows toast won't render `<image src="file:///…/foo.img" />`.
    for ext in ["png", "jpg", "jpeg", "gif", "webp", "bmp"] {
        let candidate = icons_dir.join(format!("{hash}.{ext}"));
        if candidate.exists() {
            return Ok(candidate.to_string_lossy().to_string());
        }
    }

    let parsed = reqwest::Url::parse(&url).map_err(|e| format!("invalid url: {e}"))?;
    match parsed.scheme() {
        "http" | "https" => {}
        other => return Err(format!("scheme not allowed: {other}")),
    }

    // The Matrix access token may ONLY ride along to the user's own homeserver
    // media endpoint. Anything else (or a missing/mismatched homeserver) is
    // fetched without credentials AND SSRF-guarded, so this command can neither
    // leak the token to an attacker-controlled URL nor be used to probe
    // internal services.
    let is_homeserver_media = match homeserver
        .as_deref()
        .and_then(|h| reqwest::Url::parse(h).ok())
    {
        Some(hs) => {
            hs.scheme() == parsed.scheme()
                && hs.host_str().is_some()
                && hs.host_str() == parsed.host_str()
                && hs.port_or_known_default() == parsed.port_or_known_default()
                && parsed.path().contains("/_matrix/")
        }
        None => false,
    };

    let builder = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (compatible; PrinnyNotificationIcon/1.0)");

    // The private-address guard may only be skipped for a homeserver origin the
    // application itself recorded at startup — never for one named in this
    // call's arguments.
    //
    // Both `url` and `homeserver` come from the webview, so the old test
    // ("is `homeserver` the same origin as `url`?") was satisfiable by any
    // caller: passing a matching pair for 127.0.0.1 disabled the guard outright
    // and turned this command into an internal-port scanner whose distinct error
    // strings reported whether the port was open. Comparing against state that
    // the caller cannot choose per-invocation is what makes the check mean
    // something. When no origin has been registered yet, the guard applies —
    // fail closed.
    let trusted_homeserver_origin = app
        .state::<HomeserverOrigin>()
        .0
        .lock()
        .ok()
        .and_then(|guard| guard.clone());
    let skip_private_guard = match (&trusted_homeserver_origin, parsed.host_str()) {
        (Some(origin), Some(_)) => reqwest::Url::parse(origin).is_ok_and(|hs| {
            hs.scheme() == parsed.scheme()
                && hs.host_str() == parsed.host_str()
                && hs.port_or_known_default() == parsed.port_or_known_default()
        }),
        _ => false,
    };

    let client = if skip_private_guard {
        // The user's chosen homeserver, recorded by the app at startup, which
        // may legitimately live on a LAN address.
        builder.build().map_err(|e| format!("client: {e}"))?
    } else {
        let host = parsed
            .host_str()
            .ok_or_else(|| "url has no host".to_string())?
            .to_string();
        let port = parsed.port_or_known_default().unwrap_or(443);
        let addrs = resolve_public_addrs(&host, port).await?;
        builder
            .resolve(&host, addrs[0])
            .build()
            .map_err(|e| format!("client: {e}"))?
    };

    let mut req = client.get(&url);
    if is_homeserver_media {
        if let Some(auth) = auth_header.filter(|s| !s.is_empty()) {
            req = req.header(reqwest::header::AUTHORIZATION, auth);
        }
    }
    let resp = req.send().await.map_err(|e| format!("send: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(';').next())
        .map(|s| s.trim().to_ascii_lowercase());

    // Stream with a hard cap instead of buffering the whole body. The URL here
    // is a homeserver media URL for an avatar chosen by a remote user, so
    // without a bound a sender could pick the number of bytes this process
    // allocates and then writes into the app cache directory — zero-click, just
    // by messaging the victim.
    let mut resp = resp;
    let mut bytes: Vec<u8> = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| format!("bytes: {e}"))? {
        if bytes.len() + chunk.len() > MEDIA_FETCH_MAX_BYTES {
            return Err(format!(
                "icon exceeds {MEDIA_FETCH_MAX_BYTES} byte limit"
            ));
        }
        bytes.extend_from_slice(&chunk);
    }

    let ext = match content_type.as_deref() {
        Some("image/jpeg") | Some("image/jpg") | Some("image/pjpeg") => "jpg",
        Some("image/gif") => "gif",
        Some("image/webp") => "webp",
        Some("image/bmp") => "bmp",
        Some("image/png") => "png",
        _ => match bytes.first_chunk::<4>() {
            // sniff magic bytes when the server didn't tell us
            Some([0xFF, 0xD8, 0xFF, _]) => "jpg",
            Some([0x89, 0x50, 0x4E, 0x47]) => "png",
            Some([0x47, 0x49, 0x46, _]) => "gif",
            Some([0x52, 0x49, 0x46, 0x46]) => "webp",
            Some([0x42, 0x4D, _, _]) => "bmp",
            _ => "png",
        },
    };

    let file_path = icons_dir.join(format!("{hash}.{ext}"));
    fs::write(&file_path, &bytes).map_err(|e| format!("write: {e}"))?;

    Ok(file_path.to_string_lossy().to_string())
}

// Reads a file dropped onto the window via Tauri's native drag-drop event and
// returns its bytes plus inferred MIME. Used in place of WebView2's HTML5
// DragEvent path because WebView2 hands JS zero-byte File stubs from
// dataTransfer.files on Windows — no real content reaches the page.
#[derive(serde::Serialize)]
struct DroppedFile {
    name: String,
    mime: String,
    bytes: Vec<u8>,
}

// Proxy a remote URL through Rust reqwest and return the raw bytes. We can't
// use the @tauri-apps/plugin-http path for this because its guest-js layer
// constructs a browser `Headers` object, which silently drops forbidden
// headers (User-Agent, Referer). The request then reaches reqwest with the
// default `reqwest/x.x` UA and video.twimg.com 403s it. This command sends
// a real Chrome UA and no Referer (twimg serves when Referer is absent).
#[tauri::command]
async fn fetch_remote_bytes(url: String) -> Result<tauri::ipc::Response, String> {
    let parsed = reqwest::Url::parse(&url).map_err(|e| format!("invalid url: {e}"))?;
    match parsed.scheme() {
        "http" | "https" => {}
        other => return Err(format!("scheme not allowed: {other}")),
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| "url has no host".to_string())?
        .to_string();
    if !host_allowed_media(&host) {
        return Err(format!("host not allowed: {host}"));
    }
    let port = parsed.port_or_known_default().unwrap_or(443);
    let addrs = resolve_public_addrs(&host, port).await?;

    // Redirects are disabled and re-vetted by hand, exactly as fetch_og_preview
    // does. The previous policy accepted a hop on hostname alone: the new host
    // was matched against the media allowlist but never resolved and checked
    // against is_disallowed_ip, and the DNS pin only ever covered the original
    // host, so reqwest was free to re-resolve the redirect target to anything.
    let mut current = parsed.clone();
    let mut current_addrs = addrs;
    let mut current_host = host;
    for _hop in 0..10u8 {
        let client = reqwest::Client::builder()
            .user_agent(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                 (KHTML, like Gecko) Chrome/138.0.0.0 Safari/537.36",
            )
            // Pin the vetted address so reqwest can't re-resolve to a rebound
            // internal IP between our check and the request.
            .resolve(&current_host, current_addrs[0])
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| format!("client: {e}"))?;
        let resp = client
            .get(current.as_str())
            .send()
            .await
            .map_err(|e| format!("send: {e}"))?;

        let status = resp.status();
        if status.is_redirection() {
            let loc = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| "redirect without location".to_string())?;
            let next = current
                .join(loc)
                .map_err(|e| format!("bad redirect target: {e}"))?;
            match next.scheme() {
                "http" | "https" => {}
                other => return Err(format!("scheme not allowed: {other}")),
            }
            let next_host = next
                .host_str()
                .ok_or_else(|| "redirect target has no host".to_string())?
                .to_string();
            if !host_allowed_media(&next_host) {
                return Err(format!("redirect host not allowed: {next_host}"));
            }
            let next_port = next.port_or_known_default().unwrap_or(443);
            // Re-vet and re-pin every hop, not just the first.
            current_addrs = resolve_public_addrs(&next_host, next_port).await?;
            current_host = next_host;
            current = next;
            continue;
        }

        if !status.is_success() {
            return Err(format!("HTTP {status}"));
        }

        // Cap the body. This proxies remote media chosen by message content, so
        // an unbounded read let a sender decide how much memory the native
        // process allocates.
        let mut resp = resp;
        let mut buf: Vec<u8> = Vec::new();
        while let Some(chunk) = resp.chunk().await.map_err(|e| format!("body: {e}"))? {
            if buf.len() + chunk.len() > MEDIA_FETCH_MAX_BYTES {
                return Err(format!(
                    "response exceeds {MEDIA_FETCH_MAX_BYTES} byte limit"
                ));
            }
            buf.extend_from_slice(&chunk);
        }
        return Ok(tauri::ipc::Response::new(buf));
    }
    Err("too many redirects".to_string())
}

// Maximum bytes read from an OG-preview target. OG/meta tags live in <head>,
// near the very top of the document, so a small cap is plenty and bounds the
// memory/bandwidth a single preview can cost.
const OG_PREVIEW_MAX_BYTES: usize = 512 * 1024;

// Decode the handful of HTML entities that realistically appear inside og:
// `content` attributes. Anything unrecognised is left verbatim.
fn decode_entities(input: &str) -> String {
    if !input.contains('&') {
        return input.to_string();
    }
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp..];
        let decoded = after.find(';').filter(|&semi| semi <= 10).and_then(|semi| {
            let ent = &after[1..semi];
            let ch = match ent {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" | "#39" => Some('\''),
                "nbsp" => Some('\u{00A0}'),
                _ => ent.strip_prefix('#').and_then(|num| {
                    let (radix, digits) = match num.strip_prefix(['x', 'X']) {
                        Some(hex) => (16, hex),
                        None => (10, num),
                    };
                    u32::from_str_radix(digits, radix).ok().and_then(char::from_u32)
                }),
            };
            ch.map(|c| (c, semi))
        });
        match decoded {
            Some((c, semi)) => {
                out.push(c);
                rest = &after[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &after[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

// Parse the attributes inside a single tag body (the text between `<` and `>`),
// tolerating single/double/unquoted values and arbitrary whitespace. Keys are
// lower-cased; values are returned raw (entity-decoding happens later).
fn parse_tag_attrs(tag: &str) -> std::collections::HashMap<String, String> {
    let mut attrs = std::collections::HashMap::new();
    let chars: Vec<char> = tag.chars().collect();
    let n = chars.len();
    let mut i = 0;
    // Skip the tag name (e.g. "meta").
    while i < n && !chars[i].is_whitespace() {
        i += 1;
    }
    while i < n {
        while i < n && chars[i].is_whitespace() {
            i += 1;
        }
        if i >= n {
            break;
        }
        let start = i;
        while i < n && chars[i] != '=' && !chars[i].is_whitespace() && chars[i] != '/' {
            i += 1;
        }
        let name: String = chars[start..i].iter().collect::<String>().to_ascii_lowercase();
        while i < n && chars[i].is_whitespace() {
            i += 1;
        }
        let mut value = String::new();
        if i < n && chars[i] == '=' {
            i += 1;
            while i < n && chars[i].is_whitespace() {
                i += 1;
            }
            if i < n && (chars[i] == '"' || chars[i] == '\'') {
                let quote = chars[i];
                i += 1;
                let vstart = i;
                while i < n && chars[i] != quote {
                    i += 1;
                }
                value = chars[vstart..i].iter().collect();
                if i < n {
                    i += 1;
                }
            } else {
                let vstart = i;
                while i < n && !chars[i].is_whitespace() && chars[i] != '/' {
                    i += 1;
                }
                value = chars[vstart..i].iter().collect();
            }
        }
        if !name.is_empty() {
            attrs.insert(name, value);
        }
    }
    attrs
}

// Scan an HTML document for og:/twitter:/<title> metadata and emit a map whose
// keys mirror the Matrix `preview_url` response (og:title, og:description,
// og:image, …) so the frontend renders a fallback card with zero special-casing.
fn extract_og(html: &str) -> serde_json::Map<String, serde_json::Value> {
    let lower = html.to_ascii_lowercase();
    let mut props: std::collections::HashMap<String, String> = std::collections::HashMap::new();

    let mut from = 0;
    while let Some(rel) = lower[from..].find("<meta") {
        let start = from + rel;
        let end = match html[start..].find('>') {
            Some(e) => start + e,
            None => break,
        };
        let attrs = parse_tag_attrs(&html[start + 1..end]);
        let key = attrs.get("property").or_else(|| attrs.get("name"));
        if let (Some(k), Some(c)) = (key, attrs.get("content")) {
            props.entry(k.to_ascii_lowercase()).or_insert_with(|| c.clone());
        }
        from = end + 1;
    }

    let title_tag = lower.find("<title").and_then(|ts| {
        html[ts..].find('>').and_then(|gt| {
            let content_start = ts + gt + 1;
            lower[content_start..]
                .find("</title>")
                .map(|te| html[content_start..content_start + te].trim().to_string())
        })
    });

    let mut map = serde_json::Map::new();
    let put = |map: &mut serde_json::Map<String, serde_json::Value>,
               out_key: &str,
               candidates: &[&str],
               fallback: Option<&str>| {
        for c in candidates {
            if let Some(v) = props.get(*c) {
                let dv = decode_entities(v.trim());
                if !dv.is_empty() {
                    map.insert(out_key.to_string(), serde_json::Value::String(dv));
                    return;
                }
            }
        }
        if let Some(f) = fallback {
            let f = f.trim();
            if !f.is_empty() {
                map.insert(out_key.to_string(), serde_json::Value::String(decode_entities(f)));
            }
        }
    };

    put(&mut map, "og:title", &["og:title", "twitter:title"], title_tag.as_deref());
    put(
        &mut map,
        "og:description",
        &["og:description", "twitter:description", "description"],
        None,
    );
    put(
        &mut map,
        "og:image",
        &[
            "og:image",
            "og:image:url",
            "og:image:secure_url",
            "twitter:image",
            "twitter:image:src",
        ],
        None,
    );
    put(&mut map, "og:site_name", &["og:site_name"], None);
    put(&mut map, "og:type", &["og:type"], None);
    put(&mut map, "og:video", &["og:video", "og:video:url", "og:video:secure_url"], None);
    put(&mut map, "og:image:width", &["og:image:width"], None);
    put(&mut map, "og:image:height", &["og:image:height"], None);
    map
}

// Build an OG-preview card for a generic webpage the homeserver couldn't
// preview (commonly because the target rejects non-browser User-Agents and
// Synapse's `preview_url` 504s). We fetch the page ourselves with a real Chrome
// UA — server-to-server, bypassing CORS — and parse the meta tags locally.
//
// Unlike `fetch_remote_bytes`, this accepts ANY public host (a generic preview
// can point anywhere), so it deliberately drops the media allowlist. Every
// other SSRF guard is kept: scheme check, post-DNS private/reserved-IP
// rejection, DNS pinning (no rebinding TOCTOU), per-hop redirect re-vetting
// (auto-redirects disabled — each Location is validated as a fresh request),
// and a hard response-size cap. It is gated behind an opt-in, default-off
// setting on the frontend.
#[tauri::command]
async fn fetch_og_preview(url: String) -> Result<serde_json::Value, String> {
    let mut current = reqwest::Url::parse(&url).map_err(|e| format!("invalid url: {e}"))?;
    for _hop in 0..6u8 {
        match current.scheme() {
            "http" | "https" => {}
            other => return Err(format!("scheme not allowed: {other}")),
        }
        let host = current
            .host_str()
            .ok_or_else(|| "url has no host".to_string())?
            .to_string();
        let port = current.port_or_known_default().unwrap_or(443);
        let addrs = resolve_public_addrs(&host, port).await?;

        let client = reqwest::Client::builder()
            .user_agent(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                 (KHTML, like Gecko) Chrome/138.0.0.0 Safari/537.36",
            )
            .resolve(&host, addrs[0])
            // Disable automatic redirects: we re-vet every hop (scheme +
            // public-IP + DNS pin) by re-issuing the request ourselves, so a
            // 3xx can never reach an internal host via an un-pinned re-resolve.
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| format!("client: {e}"))?;

        let resp = client
            .get(current.as_str())
            .header(
                reqwest::header::ACCEPT,
                "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
            )
            .send()
            .await
            .map_err(|e| format!("send: {e}"))?;

        let status = resp.status();
        if status.is_redirection() {
            let loc = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| "redirect without location".to_string())?;
            current = current
                .join(loc)
                .map_err(|e| format!("bad redirect target: {e}"))?;
            continue;
        }
        if !status.is_success() {
            return Err(format!("HTTP {status}"));
        }

        let mut resp = resp;
        let mut buf: Vec<u8> = Vec::new();
        while let Some(chunk) = resp.chunk().await.map_err(|e| format!("body: {e}"))? {
            buf.extend_from_slice(&chunk);
            if buf.len() >= OG_PREVIEW_MAX_BYTES {
                buf.truncate(OG_PREVIEW_MAX_BYTES);
                break;
            }
        }
        let html = String::from_utf8_lossy(&buf);
        let map = extract_og(&html);
        if map.is_empty() {
            return Err("no preview metadata found".to_string());
        }
        return Ok(serde_json::Value::Object(map));
    }
    Err("too many redirects".to_string())
}

#[tauri::command]
async fn read_dropped_file(
    state: tauri::State<'_, DroppedPaths>,
    path: String,
) -> Result<DroppedFile, String> {
    let requested = PathBuf::from(&path);
    // Resolve symlinks/.. so the comparison can't be tricked, and so it matches
    // the canonicalised form we stored on the drag-drop event.
    let canon = std::fs::canonicalize(&requested)
        .map_err(|e| format!("canonicalize {}: {}", path, e))?;

    {
        // Consume the authorisation: a drop authorises exactly one read, not an
        // indefinite licence. The set was previously never pruned, so any path
        // the user had ever dropped stayed readable by page script for the whole
        // process lifetime — including after the file's contents changed.
        let mut set = state.0.lock().map_err(|_| "drop-state poisoned".to_string())?;
        let authorised = set.remove(&canon) | set.remove(&requested);
        if !authorised {
            return Err("path was not part of a drag-drop onto the window".to_string());
        }
    }

    // Bound the read. `std::fs::read` on a caller-named path had no size limit,
    // so a single dropped file could be pulled into memory in full.
    let metadata = std::fs::metadata(&canon).map_err(|e| format!("stat {}: {}", path, e))?;
    if metadata.len() > DROPPED_FILE_MAX_BYTES as u64 {
        return Err(format!(
            "file exceeds {DROPPED_FILE_MAX_BYTES} byte limit"
        ));
    }

    let name = canon
        .file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "file".to_string());
    let mime = mime_guess::from_path(&canon)
        .first_or_octet_stream()
        .essence_str()
        .to_string();
    let bytes = std::fs::read(&canon).map_err(|e| format!("read {}: {}", path, e))?;
    Ok(DroppedFile { name, mime, bytes })
}

// Windows toast with a real `appLogoOverride` avatar in the top-left.
//
// `tauri-plugin-notification` v2.3.3 calls `notify_rust::Notification::icon()`,
// but on Windows that field is silently dropped — `notify-rust`'s Windows
// backend (windows.rs) only reads `path_to_image` and even then renders the
// image as a regular `<image id="1" src=…>` (inline below the body) rather
// than as the app logo. Building the toast directly via
// `tauri-winrt-notification` lets us emit the proper
// `<image placement="appLogoOverride" hint-crop="circle" src=…>` element so
// the avatar renders in the standard top-left position used by Discord,
// Element, Slack, etc.
//
// The activation handler emits a `notification://activated` Tauri event so
// the JS-side click listener can route the click back to the originating
// room — the same flow `tauri-plugin-notification`'s `onAction` listener
// provides on other platforms.
#[cfg(target_os = "windows")]
#[tauri::command]
fn send_windows_message_toast(
    app: tauri::AppHandle,
    title: String,
    body: String,
    icon_path: Option<String>,
    room_id: String,
    event_id: String,
    kind: String,
) -> Result<(), String> {
    use std::path::PathBuf;
    use tauri::Emitter;
    use tauri_winrt_notification::{IconCrop, Toast};

    // Must match the AppUserModelID registered by the NSIS installer
    // (`bundle.identifier` in tauri.conf.json). When this doesn't match,
    // Windows silently drops the toast.
    let app_id = app.config().identifier.clone();
    let app_handle = app.clone();

    // Showing the toast triggers WinRT activation events on the calling
    // thread. tauri-plugin-notification's desktop backend spawns a thread
    // for the same reason — see plugins/notification/src/desktop.rs in
    // the Tauri repo. Running it on the tokio executor thread directly
    // intermittently fires CO_E_NOTINITIALIZED.
    std::thread::spawn(move || {
        let mut toast = Toast::new(&app_id).title(&title).text1(&body);

        if let Some(path) = icon_path.filter(|s| !s.is_empty()) {
            let p = PathBuf::from(path);
            if p.exists() {
                toast = toast.icon(&p, IconCrop::Circular, &title);
            }
        }

        toast = toast.add_button("Open", "open");

        toast = toast.on_activated(move |_action| {
            let _ = app_handle.emit(
                "notification://activated",
                serde_json::json!({
                    "roomId": room_id,
                    "eventId": event_id,
                    "kind": kind,
                }),
            );
            Ok(())
        });

        if let Err(e) = toast.show() {
            eprintln!("[notif] tauri-winrt-notification toast.show failed: {e:?}");
        }
    });

    Ok(())
}

#[cfg(not(target_os = "windows"))]
#[tauri::command]
fn send_windows_message_toast(
    _title: String,
    _body: String,
    _icon_path: Option<String>,
    _room_id: String,
    _event_id: String,
    _kind: String,
) -> Result<(), String> {
    Err("send_windows_message_toast is Windows-only".to_string())
}

/// Asks the OS to keep this window out of screenshots and screen recordings.
///
/// Enforced by the compositor, not by us: Windows honours it via
/// `SetWindowDisplayAffinity` and macOS via `NSWindowSharingNone`. On most Linux
/// setups it does nothing at all, which is why the setting that drives this says
/// so rather than implying a guarantee we cannot make.
#[tauri::command]
fn set_content_protection(window: tauri::Window, enabled: bool) -> Result<(), String> {
    window
        .set_content_protected(enabled)
        .map_err(|e| format!("could not change content protection: {e}"))
}

#[tauri::command]
fn set_badge_count(window: tauri::Window, count: u32) {
    #[cfg(target_os = "windows")]
    {
        let idx = if count == 0 {
            None
        } else if count >= 10 {
            Some(10usize) // badge-9plus
        } else {
            Some(count as usize)
        };
        if let Ok(hwnd) = window.hwnd() {
            taskbar::set_overlay(hwnd.0 as isize, idx.map(|i| BADGE_ICONS[i]));
        }
        return;
    }

    #[cfg(not(any(target_os = "windows", target_os = "android", target_os = "ios")))]
    {
        if count > 0 {
            let _ = window.set_badge_count(Some(count.into()));
        } else {
            let _ = window.set_badge_count(None::<i64>);
        }
    }
}

/// Refuse to start if anything else already owns the frontend port.
///
/// In release builds the main webview loads http://localhost:44548, and the
/// capability files (capabilities/migrated.json, capabilities/desktop.json)
/// grant this application's native permissions to that exact origin. So whoever
/// answers on that port is handed the clipboard, an unrestricted HTTP client,
/// dialog-mediated file access, process control and the updater.
///
/// tauri-plugin-localhost starts its listener inside `std::thread::spawn` and
/// `expect`s the bind, so a panic there kills only that thread: the process
/// carries on and the webview cheerfully renders whatever the squatter serves.
/// Loopback ports are not partitioned per user on Linux or Windows, so a second
/// local user — or any process that started earlier — can take it. Binding here
/// first turns that silent takeover into a loud failure.
///
/// The port stays fixed rather than ephemeral because capability `remote.urls`
/// entries are static strings resolved at build time; an ephemeral port could
/// not be named there, and the page would end up with no capabilities at all.
///
/// WHY THIS IS A PLUGIN AND NOT A PLAIN CHECK BEFORE `Builder::default()`:
/// our own second instance also fails to bind. Run before the builder, this
/// killed every duplicate launch with exit(1) before
/// tauri-plugin-single-instance could forward the click to the window already
/// running — so clicking the taskbar pin, with the app minimised to tray,
/// appeared to do nothing whatsoever. As a plugin registered after
/// single-instance, a duplicate has already exited by the time this runs, and
/// only a genuine foreign squatter reaches the failure path.
fn port_guard_plugin<R: tauri::Runtime>(port: u16) -> tauri::plugin::TauriPlugin<R> {
    tauri::plugin::Builder::new("prinny-port-guard")
        .setup(move |_app, _api| {
            #[cfg(not(debug_assertions))]
            {
                match std::net::TcpListener::bind(("127.0.0.1", port)) {
                    Ok(probe) => drop(probe),
                    Err(e) => {
                        eprintln!(
                            "[prinny] FATAL: 127.0.0.1:{port} is already in use ({e}). The \
                             application frontend is served on this port and holds native \
                             capabilities, so refusing to start rather than load content from \
                             another process."
                        );
                        std::process::exit(1);
                    }
                }
            }
            let _ = port;
            Ok(())
        })
        .build()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // tauri-plugin-localhost serves the bundled frontend on 127.0.0.1:44548.
    // A system/corporate HTTP proxy set via env would swallow that request and
    // the window would come up blank, so make sure loopback is always excluded.
    for key in ["NO_PROXY", "no_proxy"] {
        let current_val = std::env::var(key).unwrap_or_default();
        if !current_val.contains("localhost") {
            let new_val = if current_val.is_empty() {
                "localhost,127.0.0.1".to_string()
            } else {
                format!("{},localhost,127.0.0.1", current_val)
            };
            std::env::set_var(key, new_val);
        }
    }

    // Frontend port. Fixed rather than ephemeral because capability
    // `remote.urls` entries are static strings resolved at build time. The
    // guard that stops anything else owning it lives in port_guard_plugin(),
    // which documents why it must run as a plugin rather than here.
    let port: u16 = 44548;

    let context = tauri::generate_context!();
    // Declare the same AppUserModelID the NSIS installer stamps on the Start
    // Menu shortcut (`bundle.identifier`, i.e. in.prinny.app — the value the
    // toast code below already relies on).
    //
    // Windows groups taskbar buttons by AUMID. A process that never sets one
    // gets a per-executable default that does not match the pinned shortcut,
    // so pinning the app and then running it produced TWO taskbar icons: the
    // pin, and a separate button for the live window. Setting it explicitly,
    // before any window exists, makes them one button.
    //
    // This reinforces the identity toasts already depend on rather than
    // changing it — an AUMID mismatch silently drops Windows toasts, so these
    // two must never drift apart.
    #[cfg(target_os = "windows")]
    unsafe {
        use windows::core::HSTRING;
        use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;

        // Reuses the context built above — `generate_context!()` embeds the
        // whole frontend bundle, so invoking it a second time is not free.
        let app_id = HSTRING::from(context.config().identifier.as_str());
        if let Err(e) = SetCurrentProcessExplicitAppUserModelID(&app_id) {
            eprintln!("Failed to set AppUserModelID: {e}");
        }
    }

    let mut builder = tauri::Builder::default();

    // MUST be the first plugin registered. A second instance has to bail out
    // before anything else initialises — `tauri-plugin-localhost` binds a TCP
    // port and `tauri-plugin-window-state` writes the saved geometry, so a
    // duplicate that gets as far as those either fails to start or clobbers
    // the running window's position on the way out.
    //
    // Without this, minimize-to-tray made duplicates trivial to create: the
    // close button hides the window (see useSystemTray.ts), so the app looks
    // shut down while still running, and the next click on the shortcut,
    // taskbar pin or Start menu entry launched a whole new copy — each with
    // its own tray icon and its own Matrix sync.
    #[cfg(not(mobile))]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            // Runs in the FIRST instance when a duplicate is launched.
            if let Some(window) = app.get_webview_window("main") {
                // Order matters: a window hidden to tray needs show() before
                // set_focus() does anything, and a minimized one needs
                // unminimize() as well. Both are no-ops when not applicable,
                // so run all three rather than trying to detect the state.
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }));
    }

    #[cfg(all(not(mobile), target_os = "linux"))]
    {
        builder = builder
            .manage(CaptureIntent::default())
            .manage(CaptureSession::default());
    }

    builder = builder
        .manage(rich_presence::RichPresenceBridge::default())
        .manage(DroppedPaths::default())
        .manage(HomeserverOrigin::default())
        // Record the real OS paths from each native drag-drop so that
        // `read_dropped_file` will only read files the user actually dropped.
        // Observer-only (returns unit), so it never interferes with the
        // frontend's own `onDragDropEvent` listener.
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) = event {
                if let Some(state) = window.try_state::<DroppedPaths>() {
                    if let Ok(mut set) = state.0.lock() {
                        for p in paths {
                            match std::fs::canonicalize(p) {
                                Ok(canon) => {
                                    set.insert(canon);
                                }
                                Err(_) => {
                                    set.insert(p.clone());
                                }
                            }
                        }
                    }
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            set_badge_count,
            set_homeserver_origin,
            cache_notification_icon,
            read_dropped_file,
            fetch_remote_bytes,
            fetch_og_preview,
            send_windows_message_toast,
            arm_capture_intent,
            set_capture_session,
            set_content_protection,
            rich_presence::start_rich_presence_bridge,
            rich_presence::stop_rich_presence_bridge,
        ])
        // Registered AFTER single-instance and BEFORE localhost on purpose.
        // Plugin setups run in registration order, so by the time this probes
        // the port a duplicate launch has already been intercepted and exited,
        // and tauri-plugin-localhost has not yet bound the port itself.
        .plugin(port_guard_plugin(port))
        .plugin(tauri_plugin_localhost::Builder::new(port).build())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_mobile_push::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_os::init())
        .plugin(unifiedpush_plugin())
        .plugin(foreground_plugin())
        .plugin(message_notification_plugin())
        .plugin(share_target_plugin())
        // All platforms. On Android the scheme is registered by the manifest's
        // intent filter rather than by the plugin, but the launch intent still
        // has to be read and handed to the frontend, and that is this plugin.
        // `capabilities/mobile.json` grants `deep-link:default`, so this
        // registration is also what keeps the Android build from failing on an
        // unknown permission.
        .plugin(tauri_plugin_deep_link::init());

    #[cfg(not(mobile))]
    {
        builder = builder
            // Everything EXCEPT visibility. The plugin defaults to
            // StateFlags::all(), which restores whether the window was visible
            // when it was last closed — and that fights both of the ways this
            // app legitimately hides itself:
            //   - closing to tray saves "hidden", so the next manual launch
            //     would come up invisible and look like a failure to start;
            //   - a `--minimized` autostart launch would be overridden back to
            //     visible by a restore that says the window used to be shown.
            // Size, position, maximized and fullscreen are still restored.
            .plugin(
                tauri_plugin_window_state::Builder::default()
                    .with_state_flags(
                        tauri_plugin_window_state::StateFlags::all()
                            & !tauri_plugin_window_state::StateFlags::VISIBLE,
                    )
                    .build(),
            )
            .plugin(tauri_plugin_updater::Builder::new().build())
            // Start at login, off unless the user turns it on in settings.
            // `--minimized` is passed so an app that launches itself at boot
            // does so into the tray rather than stealing the screen from
            // whatever the user actually opened their machine to do.
            .plugin(tauri_plugin_autostart::init(
                tauri_plugin_autostart::MacosLauncher::LaunchAgent,
                Some(vec!["--minimized"]),
            ));
    }

    builder
        .setup(move |app| {
            // Dev: use devUrl from tauri.conf.json (http://localhost:8080) to support HMR
            #[cfg(debug_assertions)]
            let window_url = WebviewUrl::App(Default::default());

            // Release: tauri-plugin-localhost serves bundled frontend assets on this port
            #[cfg(not(debug_assertions))]
            let window_url = {
                let url = format!("http://localhost:{}", port).parse().unwrap();
                WebviewUrl::External(url)
            };

            let app_handle = app.handle().clone();
            let mut window_builder = WebviewWindowBuilder::new(app, "main".to_string(), window_url);

            #[cfg(not(mobile))]
            {
                window_builder = window_builder.title("Cinny");
            }

            #[cfg(not(mobile))]
            {
                window_builder = window_builder.inner_size(800.0, 800.0);
            }

            // The autostart registration launches us with `--minimized`, so a
            // login-triggered start goes straight to the tray instead of
            // throwing a window in front of whatever the user actually opened
            // their machine to do. Without honouring the flag here, "start at
            // login" would mean "start and interrupt", which is not what the
            // setting says it does.
            //
            // The tray icon is what makes this recoverable, and it is created
            // unconditionally below; a hidden window with no tray icon would be
            // an app the user cannot get back.
            #[cfg(not(mobile))]
            {
                let start_hidden = std::env::args().any(|arg| arg == "--minimized");
                if start_hidden {
                    window_builder = window_builder.visible(false);
                }
            }

            // Transparent titlebar on macOS — the default is a permanently
            // white bar that ignores the app theme.
            #[cfg(target_os = "macos")]
            {
                window_builder = window_builder.title_bar_style(TitleBarStyle::Transparent);
            }

            // Keep Tauri's native drag-drop handler enabled. WebView2 (Windows)
            // hands JS zero-byte File stubs from dataTransfer.files when the OS
            // drag-drop path is bypassed, so the frontend listens for Tauri's
            // own drag-drop event (real OS paths) via useTauriDragDropListener
            // and reads bytes through read_dropped_file.

            window_builder
                .on_new_window(move |url, _features| {
                    // Only http(s) is handed to the operating system.
                    //
                    // `OpenerExt::open_url` called from Rust bypasses the
                    // capability system entirely, so the `opener:allow-open-url`
                    // scope in capabilities/migrated.json — which restricts the
                    // IPC command to http/https — does NOT constrain this call.
                    // Testing only for `blob` meant any other scheme reaching
                    // here was dispatched to whatever local application is
                    // registered for it: `file:` and UNC targets (an outbound
                    // SMB authentication on Windows), and application protocol
                    // handlers. The frontend reaches this with URLs chosen by a
                    // homeserver and by message senders, so the allowlist has to
                    // live here rather than upstream of it.
                    match url.scheme() {
                        "http" | "https" => {
                            let _ = app_handle.opener().open_url(url.as_str(), None::<&str>);
                        }
                        // blob: URLs are internal to the webview; the frontend
                        // turns those into downloads itself.
                        "blob" => {}
                        other => {
                            eprintln!("[prinny] refused to open URL with scheme {other:?}");
                        }
                    }
                    NewWindowResponse::Deny
                })
                .build()?;

            // WebKitGTK ships with media capture switched OFF, and denies every
            // permission request that nothing is connected to. Both are silent:
            // navigator.mediaDevices.getUserMedia simply rejects, which reads as
            // "our recorder is broken" rather than "the engine never offered the
            // microphone". Neither Windows nor macOS needs this — their webviews
            // enable capture and prompt on their own.
            #[cfg(all(not(mobile), target_os = "linux"))]
            {
                use webkit2gtk::glib::prelude::ObjectExt;
                use webkit2gtk::{
                    PermissionRequestExt, SettingsExt, UserMediaPermissionRequest, WebContextExt,
                    WebViewExt,
                };

                let window = app
                    .get_webview_window("main")
                    .ok_or_else(|| "main window missing".to_string())?;
                let capture_intent = app.state::<CaptureIntent>().inner() as *const CaptureIntent;
                // The state lives for the lifetime of the app, and the closure
                // runs on the main thread alongside it.
                let capture_intent = capture_intent as usize;
                let capture_session =
                    app.state::<CaptureSession>().inner() as *const CaptureSession as usize;

                window.with_webview(move |webview| {
                    let wv = webview.inner();

                    if let Some(settings) = WebViewExt::settings(&wv) {
                        settings.set_enable_media_stream(true);
                        settings.set_enable_webrtc(true);
                    }

                    // Spell checking is off by default in WebKitGTK, so the
                    // composer had no red squiggles on Linux while it did on
                    // Windows and macOS (whose webviews enable it themselves).
                    // Languages come from the user's own locale environment
                    // rather than a hardcoded list — enabling with an empty
                    // language set makes WebKit fall back to its default, which
                    // is usually en_US regardless of who is typing.
                    if let Some(context) = WebViewExt::context(&wv) {
                        let locales: Vec<String> = ["LC_ALL", "LC_MESSAGES", "LANG"]
                            .iter()
                            .filter_map(|key| std::env::var(key).ok())
                            .flat_map(|value| {
                                value
                                    .split(':')
                                    .filter(|part| !part.is_empty() && *part != "C")
                                    .map(|part| part.split('.').next().unwrap_or(part).to_string())
                                    .collect::<Vec<_>>()
                            })
                            .collect();

                        if !locales.is_empty() {
                            let refs: Vec<&str> = locales.iter().map(String::as_str).collect();
                            context.set_spell_checking_languages(&refs);
                            context.set_spell_checking_enabled(true);
                        }
                    }

                    wv.connect_permission_request(move |_wv, request| {
                        if !request.is::<UserMediaPermissionRequest>() {
                            // Geolocation, notifications, pointer lock, DRM,
                            // media-key-system: none of these are asked for by
                            // our own UI today, and a silent grant is worse than
                            // a feature that visibly does not work yet.
                            request.deny();
                            return true;
                        }

                        // SAFETY: the pointers are to Tauri-managed state that
                        // outlives the webview, and this signal runs on the main
                        // thread where that state is never moved.
                        let intent = unsafe { &*(capture_intent as *const CaptureIntent) };
                        let session = unsafe { &*(capture_session as *const CaptureSession) };

                        // A call holds the gate open for its duration; anything
                        // else has to have armed a one-shot window just before
                        // asking. The one-shot is consumed either way, so two
                        // requests never ride on one arming.
                        let in_call = session.0.lock().map(|guard| *guard).unwrap_or(false);
                        let armed = intent
                            .0
                            .lock()
                            .ok()
                            .and_then(|mut guard| guard.take())
                            .is_some_and(|at| at.elapsed() < CAPTURE_INTENT_WINDOW);

                        if in_call || armed {
                            request.allow();
                        } else {
                            eprintln!(
                                "[prinny] denied a webview capture request that our UI did not ask for"
                            );
                            request.deny();
                        }
                        true
                    });
                })?;
            }

            Ok(())
        })
        .run(context)
        .expect("error while building tauri application");
}
