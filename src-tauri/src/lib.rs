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
use tauri_plugin_opener::OpenerExt;

mod taskbar;

// Paths the user actually dropped onto the window via the OS native drag-drop
// path. `read_dropped_file` only reads paths that appear here, so a malicious
// in-page script can't invoke it with an arbitrary path (e.g. /etc/passwd).
#[derive(Default)]
struct DroppedPaths(Mutex<HashSet<PathBuf>>);

// ---- SSRF / remote-fetch guards -------------------------------------------

// Media hosts our frontend legitimately proxies through `fetch_remote_bytes`
// (Twitter/X CDN via vxtwitter, Bluesky video/image CDN). Suffix-matched, so
// every subdomain (video.twimg.com, pbs.twimg.com, video.bsky.app,
// cdn.bsky.app, …) is covered. Keep this list tight — it is the allowlist that
// stops the command being used as a generic SSRF primitive.
const ALLOWED_MEDIA_HOSTS: &[&str] = &["twimg.com", "bsky.app"];

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
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return is_disallowed_ip(&IpAddr::V4(mapped));
            }
            let seg = v6.segments();
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

    let client = if is_homeserver_media {
        // Trusted destination (the user's chosen homeserver — which may legitimately
        // live on a LAN/private IP), so skip the private-address guard here.
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
    let bytes = resp.bytes().await.map_err(|e| format!("bytes: {e}"))?;

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

    let client = reqwest::Client::builder()
        .user_agent(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/138.0.0.0 Safari/537.36",
        )
        // Pin the vetted address so reqwest can't re-resolve to a rebound
        // internal IP between our check and the request.
        .resolve(&host, addrs[0])
        // Only follow redirects that stay within the media allowlist; a 3xx to
        // any other host is stopped (the caller gets the 3xx and errors out)
        // so a redirect can't be used to reach an internal host.
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            match attempt.url().host_str() {
                Some(h) if host_allowed_media(h) => {
                    if attempt.previous().len() >= 10 {
                        attempt.error("too many redirects")
                    } else {
                        attempt.follow()
                    }
                }
                _ => attempt.stop(),
            }
        }))
        .build()
        .map_err(|e| format!("client: {e}"))?;
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("send: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let bytes = resp.bytes().await.map_err(|e| format!("bytes: {e}"))?;
    Ok(tauri::ipc::Response::new(bytes.to_vec()))
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
        let set = state.0.lock().map_err(|_| "drop-state poisoned".to_string())?;
        if !(set.contains(&canon) || set.contains(&requested)) {
            return Err("path was not part of a drag-drop onto the window".to_string());
        }
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let port: u16 = 44548;
    let context = tauri::generate_context!();
    let mut builder = tauri::Builder::default()
        .manage(DroppedPaths::default())
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
            cache_notification_icon,
            read_dropped_file,
            fetch_remote_bytes,
            fetch_og_preview,
            send_windows_message_toast,
        ])
        .plugin(tauri_plugin_localhost::Builder::new(port).build())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_mobile_push::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_http::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(unifiedpush_plugin())
        .plugin(foreground_plugin())
        .plugin(message_notification_plugin());

    #[cfg(not(mobile))]
    {
        builder = builder
            .plugin(tauri_plugin_window_state::Builder::default().build())
            .plugin(tauri_plugin_updater::Builder::new().build());
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

            // Keep Tauri's native drag-drop handler enabled. WebView2 (Windows)
            // hands JS zero-byte File stubs from dataTransfer.files when the OS
            // drag-drop path is bypassed, so the frontend listens for Tauri's
            // own drag-drop event (real OS paths) via useTauriDragDropListener
            // and reads bytes through read_dropped_file.

            window_builder
                .on_new_window(move |url, _features| {
                    // blob: URLs are internal to the webview, skip external open
                    if url.scheme() != "blob" {
                        let _ = app_handle.opener().open_url(url.as_str(), None::<&str>);
                    }
                    NewWindowResponse::Deny
                })
                .build()?;
            Ok(())
        })
        .run(context)
        .expect("error while building tauri application");
}
