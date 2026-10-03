//! Reddit post media for the inline embed (`utils/reddit.ts` in the frontend).
//!
//! Why this lives in Rust and not beside the Twitter/Bluesky/Rule34 fetchers in
//! the page — all of it measured against the live site on 03.10.2026, from a
//! residential connection, in curl AND in a real Chromium:
//!
//!  - `www.reddit.com/…/.json` (and `old.`, `api.`, `/by_id/`) answers a
//!    logged-out client with `403` and an HTML page reading "You've been
//!    blocked by network security". The post's own HTML page serves a "Prove
//!    your humanity" challenge. So the well-known `.json` trick is not
//!    something a client can rely on any more; it is still tried, second, for
//!    the addresses Reddit has not flagged.
//!  - `embed.reddit.com/r/{sub}/comments/{id}/` — the page Reddit's own embed
//!    widget frames — answers 200 with the post's media in the markup: the HLS
//!    playlist, muxed MP4s *with audio* at every resolution, every gallery
//!    image, the image itself, plus a structured `shreddit-screenview-data`
//!    JSON blob naming the post id, type, subreddit and NSFW flag.
//!  - None of those pages, nor `/oembed`, sends `Access-Control-Allow-Origin`,
//!    so an in-page `fetch` can never read them. Off the webview there is no
//!    CORS, which is the whole reason for this command.
//!
//! The media itself is a different story and needs none of this: `v.redd.it`
//! sends `access-control-allow-origin: *`, and `i.redd.it`, `preview.redd.it`
//! and `packaged-media.redd.it` serve a cross-origin `Referer` without
//! complaint, so every URL returned here goes straight into an element.
//!
//! This is deliberately NOT a general-purpose fetcher. The page hands over a
//! parsed target (post id, subreddit, share token) — never a URL — and every
//! request URL is built here from validated parts against two fixed origins.
//! Every media URL in the answer is re-checked to be https on a `redd.it` host
//! before it is returned, because it comes from third-party markup and ends up
//! in a `src`.

use serde::{Deserialize, Serialize};

use super::{decode_entities, parse_tag_attrs, resolve_public_addrs};

const EMBED_ORIGIN: &str = "https://embed.reddit.com";
const WWW_ORIGIN: &str = "https://www.reddit.com";

/// The only hosts a request from this module may reach, redirects included.
const FETCH_HOSTS: &[&str] = &["embed.reddit.com", "www.reddit.com", "reddit.com"];

/// Suffix every returned media URL's host must carry (`i.redd.it`,
/// `v.redd.it`, `preview.redd.it`, `external-preview.redd.it`,
/// `packaged-media.redd.it`).
const MEDIA_HOST_SUFFIX: &str = "redd.it";

// An embed page measured 300–345 KB; a post `.json` with no comments is a
// fraction of that. Both caps leave an order of magnitude of headroom and still
// bound what a hostile or broken answer can make this process allocate.
const EMBED_MAX_BYTES: usize = 4 * 1024 * 1024;
const JSON_MAX_BYTES: usize = 4 * 1024 * 1024;

/// The subreddit asked for when a link names none (`redd.it/{id}`,
/// `/comments/{id}`). The embed route requires one, and measured: any REAL
/// subreddit serves any post in full — `r/pics/comments/1ww1h11` renders the
/// r/whatisit video — while `r/all` and a nonexistent name both answer "Not
/// supported post". The page's permalink then names the true subreddit, so
/// this never leaks into the result. `pics` because it is large and
/// permanent.
const PLACEHOLDER_SUBREDDIT: &str = "pics";

const MAX_REDIRECTS: u8 = 5;
const REQUEST_TIMEOUT_SECS: u64 = 12;

// The same UA `fetch_remote_bytes` and `fetch_og_preview` send. Measured: the
// embed page answers it (and even `reqwest/0.12`) with 200; the `.json` block
// is address-based and no UA changes it.
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                          (KHTML, like Gecko) Chrome/138.0.0.0 Safari/537.36";

/// Prefixes on the error string the frontend reads to decide whether asking
/// again could help. Anything without one is treated as transient.
const ERR_GONE: &str = "gone:";
const ERR_NO_MEDIA: &str = "nomedia:";
const ERR_INVALID: &str = "invalid:";

/* -------------------------------------------------------------------------- */
/* Request / response shapes                                                   */
/* -------------------------------------------------------------------------- */

/// What the page asks for. Parsed out of the link by the frontend; validated
/// again here because the page is not trusted to have done it.
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum RedditTarget {
    /// `/r/{sub}/comments/{id}`, `/comments/{id}`, `redd.it/{id}`.
    Post { id: String, subreddit: Option<String> },
    /// `/r/{sub}/s/{token}` — the app's share link, which names no post until
    /// Reddit redirects it.
    Share { subreddit: String, token: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaKind {
    /// A still image.
    Image,
    /// An animated image file (`.gif`) — drawn by an `<img>`.
    Gif,
    /// A silent looping MP4 standing in for a GIF.
    Clip,
    /// A real video: controls, no autoplay, usually with sound.
    Video,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoSource {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RedditMedia {
    pub kind: MediaKind,
    /// The file to show. For a video this is the best muxed MP4 when Reddit
    /// made one, else the HLS playlist.
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail_url: Option<String>,
    /// The stable HLS playlist of a video. Unsigned, so it outlives the
    /// signed MP4 URLs — the frontend falls back to it when those expire.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hls_url: Option<String>,
    /// Muxed MP4 renditions, smallest first.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<VideoSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_secs: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PostSource {
    Embed,
    Json,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RedditPost {
    /// Base-36 post id, without the `t3_` prefix.
    pub id: String,
    pub subreddit: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment_count: Option<i64>,
    pub nsfw: bool,
    /// Epoch milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<i64>,
    pub permalink: String,
    pub media: Vec<RedditMedia>,
    /// Epoch milliseconds at which the earliest signed media URL stops
    /// working, when any of them is signed. The frontend's cache honours it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
    /// Which of the two sources answered — reported so a console line can say
    /// so, since they fail for entirely different reasons.
    pub source: PostSource,
}

/* -------------------------------------------------------------------------- */
/* Validation                                                                  */
/* -------------------------------------------------------------------------- */

/// Base-36 post id. Seven characters today; bounded generously.
fn valid_post_id(id: &str) -> bool {
    (1..=12).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_digit() || b.is_ascii_lowercase())
}

/// A media id — the `{id}` of `i.redd.it/{id}.jpg` or `v.redd.it/{id}/…`.
/// Measured at 13 characters, one longer than a post id, so it gets its own
/// bound rather than borrowing that one.
fn valid_media_id(id: &str) -> bool {
    (1..=20).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_digit() || b.is_ascii_lowercase())
}

/// Subreddit names are 3–21 of `[A-Za-z0-9_]`; a user profile's is `u_name`,
/// and usernames run to 20 characters of the same plus `-`.
fn valid_subreddit(sub: &str) -> bool {
    (2..=32).contains(&sub.len())
        && sub
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn valid_share_token(token: &str) -> bool {
    (1..=32).contains(&token.len()) && token.bytes().all(|b| b.is_ascii_alphanumeric())
}

fn valid_username(name: &str) -> bool {
    (1..=32).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// `value` as a media URL safe to hand to an element, or None.
///
/// Entity-decoded first: these come out of HTML attributes, where `&` is
/// written `&amp;`, and a signed URL with its separators still encoded is a
/// URL whose signature no longer matches.
fn media_url(value: &str) -> Option<String> {
    let decoded = decode_entities(value.trim());
    let parsed = reqwest::Url::parse(&decoded).ok()?;
    if parsed.scheme() != "https" {
        return None;
    }
    let host = parsed.host_str()?.trim_end_matches('.').to_ascii_lowercase();
    if host != MEDIA_HOST_SUFFIX && !host.ends_with(&format!(".{MEDIA_HOST_SUFFIX}")) {
        return None;
    }
    Some(parsed.to_string())
}

fn host_of(url: &str) -> Option<String> {
    reqwest::Url::parse(url)
        .ok()?
        .host_str()
        .map(|h| h.trim_end_matches('.').to_ascii_lowercase())
}

/// The lower-cased extension of a URL's last path segment.
fn url_extension(url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(url).ok()?;
    let last = parsed.path_segments()?.next_back()?.to_string();
    let (_, ext) = last.rsplit_once('.')?;
    Some(ext.to_ascii_lowercase())
}

/// Still vs animated, from the file's extension — the only signal an image
/// URL carries.
fn image_kind(url: &str) -> MediaKind {
    match url_extension(url).as_deref() {
        Some("gif") => MediaKind::Gif,
        _ => MediaKind::Image,
    }
}

/// The `e=` expiry (epoch seconds) a `packaged-media.redd.it` URL is signed
/// until, in epoch milliseconds.
fn signed_expiry_ms(url: &str) -> Option<i64> {
    let parsed = reqwest::Url::parse(url).ok()?;
    let (_, value) = parsed.query_pairs().find(|(k, _)| k == "e")?;
    value.parse::<i64>().ok().filter(|s| *s > 0).map(|s| s * 1000)
}

/// The video id in any `v.redd.it/{vid}/…` URL.
fn vreddit_id(url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(url).ok()?;
    if parsed.host_str()? != "v.redd.it" {
        return None;
    }
    let vid = parsed.path_segments()?.next()?.to_string();
    valid_media_id(&vid).then_some(vid)
}

/// The unsigned master playlist for a video id. Measured to answer 200 with
/// no query string at all, so unlike the signed `?a=` form the embed page
/// carries, it does not expire.
fn stable_hls_url(vid: &str) -> String {
    format!("https://v.redd.it/{vid}/HLSPlaylist.m3u8")
}

fn permalink(sub: &str, id: &str) -> String {
    format!("{WWW_ORIGIN}/r/{sub}/comments/{id}/")
}

/// `(subreddit, id)` from a Reddit post path: `/r/{sub}/comments/{id}/…`,
/// `/user/{name}/comments/{id}/…` (a profile post — subreddit `u_{name}`), or
/// `/comments/{id}`.
fn parse_post_path(path: &str) -> Option<(Option<String>, String)> {
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let (sub, rest) = match segments.as_slice() {
        ["r", sub, rest @ ..] => (Some((*sub).to_string()), rest),
        ["user" | "u", name, rest @ ..] => (Some(format!("u_{name}")), rest),
        rest => (None, rest),
    };
    let id = match rest {
        ["comments", id, ..] => id.to_ascii_lowercase(),
        _ => return None,
    };
    if !valid_post_id(&id) {
        return None;
    }
    let sub = sub.filter(|s| valid_subreddit(s));
    Some((sub, id))
}

/* -------------------------------------------------------------------------- */
/* Transport                                                                   */
/* -------------------------------------------------------------------------- */

struct Answer {
    status: reqwest::StatusCode,
    location: Option<String>,
    body: String,
}

/// One GET, no redirects followed, against a host in `FETCH_HOSTS` only.
///
/// The host list is fixed, but the request still goes through the same
/// resolve-vet-pin sequence every other outbound fetch in this shell uses, so
/// a poisoned resolver cannot point it at an internal address either.
async fn get_once(url: &reqwest::Url, accept: &str, max_bytes: usize) -> Result<Answer, String> {
    if url.scheme() != "https" {
        return Err(format!("scheme not allowed: {}", url.scheme()));
    }
    let host = url.host_str().ok_or("url has no host")?.to_string();
    if !FETCH_HOSTS.contains(&host.as_str()) {
        return Err(format!("host not allowed: {host}"));
    }
    let addrs = resolve_public_addrs(&host, 443).await?;

    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        // Load-bearing. Reddit's edge fingerprints the TLS ClientHello, and
        // rustls's TLS 1.3 hello is on its block list: measured 03.10.2026,
        // `embed.reddit.com` answered rustls 403 "You've been blocked by
        // network security." under five different User-Agents, and 200 to the
        // identical request capped at TLS 1.2 — and to curl/OpenSSL at 1.3.
        // Neither headers nor HTTP version changed the answer. native-tls
        // would also pass, but it drags OpenSSL into the Android build.
        .max_tls_version(reqwest::tls::Version::TLS_1_2)
        .resolve(&host, addrs[0])
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS))
        .build()
        .map_err(|e| format!("client: {e}"))?;
    let mut resp = client
        .get(url.as_str())
        .header(reqwest::header::ACCEPT, accept)
        .send()
        .await
        .map_err(|e| format!("send {host}: {e}"))?;

    let status = resp.status();
    let location = resp
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    if status.is_redirection() {
        return Ok(Answer { status, location, body: String::new() });
    }

    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| format!("body {host}: {e}"))? {
        if buf.len() + chunk.len() > max_bytes {
            return Err(format!("{host} answer exceeds {max_bytes} bytes"));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(Answer { status, location, body: String::from_utf8_lossy(&buf).into_owned() })
}

/// GET, following redirects by hand while they stay on `FETCH_HOSTS`.
async fn get(url: &str, accept: &str, max_bytes: usize) -> Result<Answer, String> {
    let mut current = reqwest::Url::parse(url).map_err(|e| format!("bad url: {e}"))?;
    for _ in 0..=MAX_REDIRECTS {
        let answer = get_once(&current, accept, max_bytes).await?;
        if !answer.status.is_redirection() {
            return Ok(answer);
        }
        let location = answer.location.ok_or("redirect without location")?;
        current = current
            .join(&location)
            .map_err(|e| format!("bad redirect target: {e}"))?;
    }
    Err("too many redirects".to_string())
}

/// Follow a share link to the post it names.
///
/// Measured: the redirect is served even from an address the `.json`
/// endpoint blocks, so only the `Location` headers are read — never a body.
/// An unknown token redirects to the subreddit's front page, which is not a
/// post and is reported as gone.
async fn resolve_share(sub: &str, token: &str) -> Result<(Option<String>, String), String> {
    let mut current = reqwest::Url::parse(&format!("{WWW_ORIGIN}/r/{sub}/s/{token}"))
        .map_err(|e| format!("bad url: {e}"))?;
    for _ in 0..=MAX_REDIRECTS {
        let answer = get_once(&current, "text/html", EMBED_MAX_BYTES).await?;
        if !answer.status.is_redirection() {
            return Err(format!(
                "{ERR_GONE} share link answered {} without naming a post",
                answer.status
            ));
        }
        let location = answer.location.ok_or("redirect without location")?;
        current = current
            .join(&location)
            .map_err(|e| format!("bad redirect target: {e}"))?;
        if let Some(found) = parse_post_path(current.path()) {
            return Ok(found);
        }
        if current.path().contains("/s/") {
            continue;
        }
        return Err(format!("{ERR_GONE} share link resolved to {} (not a post)", current.path()));
    }
    Err("share link: too many redirects".to_string())
}

/* -------------------------------------------------------------------------- */
/* Embed page parsing                                                          */
/* -------------------------------------------------------------------------- */

#[derive(Debug, PartialEq)]
enum EmbedError {
    /// The post is deleted, removed or does not exist. Final.
    Gone(String),
    /// The post exists and carries nothing this embed can show. Final.
    NoMedia(String),
    /// The page did not look like an embed of this post — the markup moved,
    /// or Reddit served something else. Worth trying the `.json` route.
    Unrecognised(String),
}

/// Where the tag starting at `start` (its `<`) ends — the index of its `>` —
/// skipping any `>` inside a quoted attribute value.
fn tag_end(html: &str, start: usize) -> Option<usize> {
    let mut quote: Option<u8> = None;
    for (offset, byte) in html.as_bytes()[start..].iter().enumerate() {
        match (quote, *byte) {
            (Some(q), b) if b == q => quote = None,
            (Some(_), _) => {}
            (None, b'"') | (None, b'\'') => quote = Some(*byte),
            (None, b'>') => return Some(start + offset),
            _ => {}
        }
    }
    None
}

/// The first `<name …>` tag at or after `from`: its start, its end, and its
/// attributes (raw, not yet entity-decoded).
fn find_tag(
    html: &str,
    from: usize,
    name: &str,
) -> Option<(usize, usize, std::collections::HashMap<String, String>)> {
    let needle = format!("<{name}");
    let mut search = from;
    while let Some(rel) = html.get(search..)?.find(&needle) {
        let start = search + rel;
        let after = html.as_bytes().get(start + needle.len()).copied();
        // `<img` must not match `<imgsomething`; a tag name ends at whitespace,
        // `>` or `/`.
        if matches!(after, Some(b' ' | b'\t' | b'\n' | b'\r' | b'>' | b'/')) {
            let end = tag_end(html, start)?;
            return Some((start, end, parse_tag_attrs(&html[start + 1..end])));
        }
        search = start + needle.len();
    }
    None
}

fn attr(attrs: &std::collections::HashMap<String, String>, key: &str) -> Option<String> {
    attrs
        .get(key)
        .map(|v| decode_entities(v))
        .filter(|v| !v.is_empty())
}

/// Visible text between two offsets: tags dropped, entities decoded,
/// whitespace collapsed.
fn visible_text(fragment: &str) -> String {
    let mut out = String::with_capacity(fragment.len());
    let mut in_tag = false;
    for ch in fragment.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    decode_entities(&out).split_whitespace().collect::<Vec<_>>().join(" ")
}

fn bounded(text: String, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text;
    }
    let mut cut: String = text.chars().take(max_chars).collect();
    cut.push('…');
    cut
}

/// The `shreddit-screenview-data` blob: Reddit's own analytics context for
/// the page, and the one part of it that is structured data rather than
/// presentation. `{"post":{"id":"t3_…","url":…,"nsfw":…,"created_timestamp":…,
/// "type":"video"|"image"|"gallery"|…},"subreddit":{"name":…}}`.
#[derive(Debug, Default)]
struct ScreenView {
    post_id: Option<String>,
    post_url: Option<String>,
    post_type: Option<String>,
    nsfw: bool,
    created_at: Option<i64>,
    subreddit: Option<String>,
    /// Byte offset just past the tag, where the post's own markup begins.
    body_start: usize,
}

fn screenview(html: &str) -> Option<ScreenView> {
    let (_, end, attrs) = find_tag(html, 0, "shreddit-screenview-data")?;
    let data: serde_json::Value = serde_json::from_str(&attr(&attrs, "data")?).ok()?;
    let post = data.get("post")?;
    Some(ScreenView {
        post_id: post.get("id").and_then(|v| v.as_str()).map(str::to_string),
        post_url: post.get("url").and_then(|v| v.as_str()).map(str::to_string),
        post_type: post.get("type").and_then(|v| v.as_str()).map(str::to_string),
        nsfw: post.get("nsfw").and_then(|v| v.as_bool()).unwrap_or(false),
        created_at: post.get("created_timestamp").and_then(|v| v.as_i64()),
        subreddit: data
            .pointer("/subreddit/name")
            .and_then(|v| v.as_str())
            .filter(|s| valid_subreddit(s))
            .map(str::to_string),
        body_start: end + 1,
    })
}

/// The video in a `<shreddit-player>` tag.
///
/// `packaged-media-json` lists Reddit's muxed MP4s — audio and video in one
/// file, measured to carry an `soun` track and to answer ranged requests with
/// 206 — so a plain `<video src>` plays them with sound and seeking. They are
/// signed (`e=` a few hours out), which is why the stable playlist travels
/// with them.
fn player_media(attrs: &std::collections::HashMap<String, String>) -> Option<RedditMedia> {
    let player_src = attr(attrs, "src").and_then(|s| media_url(&s));
    let vid = player_src.as_deref().and_then(vreddit_id);
    let hls_url = match (&vid, &player_src) {
        (Some(vid), _) => Some(stable_hls_url(vid)),
        (None, Some(src)) => Some(src.clone()),
        (None, None) => None,
    };

    let mut sources: Vec<VideoSource> = Vec::new();
    let mut duration_secs = None;
    if let Some(packaged) = attr(attrs, "packaged-media-json")
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
    {
        duration_secs = packaged.pointer("/playbackMp4s/duration").and_then(|v| v.as_f64());
        if let Some(perms) = packaged
            .pointer("/playbackMp4s/permutations")
            .and_then(|v| v.as_array())
        {
            for perm in perms {
                let Some(url) = perm
                    .pointer("/source/url")
                    .and_then(|v| v.as_str())
                    .and_then(media_url)
                else {
                    continue;
                };
                let dim = |k: &str| {
                    perm.pointer(&format!("/source/dimensions/{k}"))
                        .and_then(|v| v.as_u64())
                        .and_then(|n| u32::try_from(n).ok())
                        .filter(|n| *n > 0)
                };
                sources.push(VideoSource { url, width: dim("width"), height: dim("height") });
            }
        }
    }
    sources.sort_by_key(|s| s.height.unwrap_or(0));
    sources.dedup_by(|a, b| a.url == b.url);

    let best = sources.last().cloned();
    let url = best.as_ref().map(|s| s.url.clone()).or_else(|| hls_url.clone())?;
    Some(RedditMedia {
        kind: MediaKind::Video,
        url,
        width: best.as_ref().and_then(|s| s.width),
        height: best.as_ref().and_then(|s| s.height),
        thumbnail_url: attr(attrs, "poster").and_then(|p| media_url(&p)),
        hls_url,
        sources,
        duration_secs,
        caption: None,
    })
}

/// A gallery item's original file, from the preview URL the carousel shows.
///
/// The carousel renders `preview.redd.it/{slug}-v0-{media_id}.{ext}?width=640…`
/// — a downscale. The original is `i.redd.it/{media_id}.{ext}`, measured to
/// answer 200 for every item tried; the `.json` route names the same id as the
/// key of `media_metadata`.
fn gallery_original(preview: &str) -> Option<(String, String)> {
    let parsed = reqwest::Url::parse(preview).ok()?;
    let last = parsed.path_segments()?.next_back()?.to_string();
    let (stem, ext) = last.rsplit_once('.')?;
    let media_id = stem.rsplit_once("-v0-").map(|(_, id)| id).unwrap_or(stem);
    if !valid_media_id(media_id) || !ext.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return None;
    }
    let ext = ext.to_ascii_lowercase();
    Some((media_id.to_string(), format!("https://i.redd.it/{media_id}.{ext}")))
}

fn gallery_media(html: &str, from: usize) -> Vec<RedditMedia> {
    let Some((start, _, _)) = find_tag(html, from, "gallery-carousel") else {
        return Vec::new();
    };
    let end = html[start..]
        .find("</gallery-carousel>")
        .map(|e| start + e)
        .unwrap_or(html.len());
    let block = &html[..end];

    let mut seen = std::collections::HashSet::new();
    let mut media = Vec::new();
    let mut cursor = start;
    // `<img>` only: each slide also carries a blurred `<faceplate-img>`
    // backdrop of the same picture, which is not a separate item.
    while let Some((_, tag_end, attrs)) = find_tag(block, cursor, "img") {
        cursor = tag_end + 1;
        let Some(preview) = attr(&attrs, "src").and_then(|s| media_url(&s)) else {
            continue;
        };
        let Some((media_id, original)) = gallery_original(&preview) else {
            continue;
        };
        if !seen.insert(media_id) {
            continue;
        }
        media.push(RedditMedia {
            kind: image_kind(&original),
            url: original,
            width: None,
            height: None,
            thumbnail_url: Some(preview),
            hls_url: None,
            sources: Vec::new(),
            duration_secs: None,
            caption: None,
        });
    }
    media
}

/// The first preview image inside the post body — the still an image post
/// whose file lives off `i.redd.it` is shown with.
fn first_preview_image(html: &str, from: usize) -> Option<String> {
    let mut cursor = from;
    while let Some((_, end, attrs)) = find_tag(html, cursor, "img") {
        cursor = end + 1;
        if attr(&attrs, "alt").as_deref() == Some("Media error") {
            continue;
        }
        if let Some(src) = attr(&attrs, "src").and_then(|s| media_url(&s)) {
            if host_of(&src).is_some_and(|h| h.ends_with("preview.redd.it")) {
                return Some(src);
            }
        }
    }
    None
}

/// The number in `<faceplate-number number="N">` when the text after it
/// starts with `word` ("upvote" / "upvotes").
fn labelled_number(html: &str, from: usize, word: &str) -> Option<i64> {
    let mut cursor = from;
    while let Some((_, end, attrs)) = find_tag(html, cursor, "faceplate-number") {
        cursor = end + 1;
        let tail = html[cursor..]
            .strip_prefix("</faceplate-number>")
            .unwrap_or(&html[cursor..]);
        if tail.trim_start().starts_with(word) {
            return attr(&attrs, "number").and_then(|n| n.parse().ok());
        }
    }
    None
}

/// `N` in the action bar's "View N comments".
///
/// Every "View " is tried: the page has others ("View on Reddit") ahead of it.
fn comment_count(text: &str) -> Option<i64> {
    text.match_indices("View ").find_map(|(at, needle)| {
        let tail = &text[at + needle.len()..];
        let digits: String = tail
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == ',')
            .filter(char::is_ascii_digit)
            .collect();
        let rest = tail.trim_start_matches(|c: char| c.is_ascii_digit() || c == ',');
        if digits.is_empty() || !rest.trim_start().starts_with("comment") {
            return None;
        }
        digits.parse().ok()
    })
}

/// `(subreddit, id)` from the `<shreddit-embed-copy-link-button permalink=…>`.
fn embed_permalink(html: &str) -> Option<(Option<String>, String)> {
    let (_, _, attrs) = find_tag(html, 0, "shreddit-embed-copy-link-button")?;
    let url = reqwest::Url::parse(&attr(&attrs, "permalink")?).ok()?;
    if !FETCH_HOSTS.contains(&url.host_str()?) {
        return None;
    }
    parse_post_path(url.path())
}

fn author_from_embed(html: &str, from: usize) -> Option<String> {
    const MARKER: &str = "href=\"https://www.reddit.com/user/";
    let at = html[from..].find(MARKER)? + from + MARKER.len();
    let name: String = html[at..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .collect();
    valid_username(&name).then_some(name)
}

fn title_from_embed(html: &str, from: usize) -> Option<String> {
    let (_, end, _) = find_tag(html, from, "h1")?;
    let close = html[end..].find("</h1>")? + end;
    let text = visible_text(&html[end + 1..close]);
    (!text.is_empty()).then(|| bounded(text, 300))
}

/// Read an `embed.reddit.com` page for post `id`.
///
/// Media is detected from the markup first (`<shreddit-player>`,
/// `<gallery-carousel>`) and from the screenview's `type` only second, so a
/// crosspost — whose type need not be the parent's — still yields the media
/// Reddit actually rendered.
fn parse_embed(html: &str, id: &str) -> Result<RedditPost, EmbedError> {
    let view = match screenview(html) {
        Some(view) => view,
        None => {
            let text = visible_text(html);
            if text.contains("has been deleted") || text.contains("has been removed") {
                return Err(EmbedError::Gone("post deleted or removed".into()));
            }
            return Err(EmbedError::Unrecognised("no shreddit-screenview-data".into()));
        }
    };

    let expected = format!("t3_{id}");
    if view.post_id.as_deref() != Some(expected.as_str()) {
        return Err(EmbedError::Unrecognised(format!(
            "page describes {:?}, not {expected}",
            view.post_id
        )));
    }
    // The screenview's subreddit is NOT the post's: it echoes whichever one
    // the request URL named (`r/all` reports `all`, `r/pics` reports `pics`),
    // and any real subreddit serves any post. The copy-link button's
    // permalink is the one place the page states where the post lives.
    let subreddit = match embed_permalink(html) {
        Some((Some(sub), permalink_id)) if permalink_id == id => sub,
        Some((_, permalink_id)) if permalink_id != id => {
            return Err(EmbedError::Unrecognised(format!(
                "permalink names {permalink_id}, not {id}"
            )));
        }
        _ => view
            .subreddit
            .clone()
            .filter(|s| s != "all")
            .ok_or_else(|| EmbedError::Unrecognised("no subreddit on the page".into()))?,
    };

    let body = view.body_start;
    let body_text = visible_text(&html[body..]);
    if body_text.contains("This post has been deleted") || body_text.contains("This post has been removed") {
        return Err(EmbedError::Gone("post deleted or removed".into()));
    }
    // What `r/all/comments/{id}` answers: a real screenview naming the post's
    // actual subreddit, and no post. Not an answer about the media.
    if body_text.contains("Not supported post") {
        return Err(EmbedError::Unrecognised("embed refused: not supported post".into()));
    }

    let mut media: Vec<RedditMedia> = Vec::new();
    if let Some((_, _, attrs)) = find_tag(html, body, "shreddit-player") {
        media.extend(player_media(&attrs));
    }
    if media.is_empty() {
        media = gallery_media(html, body);
    }
    if media.is_empty() {
        let direct = view
            .post_url
            .as_deref()
            .and_then(media_url)
            .filter(|u| host_of(u).as_deref() == Some("i.redd.it"));
        let preview = first_preview_image(html, body);
        if let Some(url) = direct.or_else(|| {
            matches!(view.post_type.as_deref(), Some("image") | Some("gif"))
                .then(|| preview.clone())
                .flatten()
        }) {
            media.push(RedditMedia {
                kind: image_kind(&url),
                thumbnail_url: preview.filter(|p| *p != url),
                url,
                width: None,
                height: None,
                hls_url: None,
                sources: Vec::new(),
                duration_secs: None,
                caption: None,
            });
        }
    }

    if media.is_empty() {
        let kind = view.post_type.as_deref().unwrap_or("unknown");
        // A post Reddit says is media-bearing but whose media this parser
        // could not find means the markup moved — worth the second route.
        if matches!(kind, "video" | "image" | "gallery" | "gif") {
            return Err(EmbedError::Unrecognised(format!("{kind} post, no media found in markup")));
        }
        return Err(EmbedError::NoMedia(format!("{kind} post")));
    }

    let expires_at = media
        .iter()
        .flat_map(|m| std::iter::once(&m.url).chain(m.sources.iter().map(|s| &s.url)))
        .filter_map(|u| signed_expiry_ms(u))
        .min();

    Ok(RedditPost {
        id: id.to_string(),
        permalink: permalink(&subreddit, id),
        title: title_from_embed(html, body),
        author: author_from_embed(html, body),
        score: labelled_number(html, body, "upvote"),
        comment_count: comment_count(&body_text),
        nsfw: view.nsfw,
        created_at: view.created_at,
        subreddit,
        media,
        expires_at,
        source: PostSource::Embed,
    })
}

/* -------------------------------------------------------------------------- */
/* .json parsing                                                               */
/* -------------------------------------------------------------------------- */

fn json_str<'a>(v: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(|x| x.as_str()).filter(|s| !s.is_empty())
}

fn json_dim(v: &serde_json::Value, key: &str) -> Option<u32> {
    v.get(key)
        .and_then(|x| x.as_u64())
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| *n > 0)
}

/// `preview.images[0].source` of a post: its still, with dimensions.
fn json_preview(post: &serde_json::Value) -> Option<(String, Option<u32>, Option<u32>)> {
    let source = post.pointer("/preview/images/0/source")?;
    let url = json_str(source, "url").and_then(media_url)?;
    Some((url, json_dim(source, "width"), json_dim(source, "height")))
}

fn json_video(post: &serde_json::Value) -> Option<RedditMedia> {
    let video = post
        .pointer("/secure_media/reddit_video")
        .or_else(|| post.pointer("/media/reddit_video"))
        .filter(|v| v.is_object());
    let preview_video = post.pointer("/preview/reddit_video_preview").filter(|v| v.is_object());
    let (video, is_clip) = match (video, preview_video) {
        (Some(v), _) => (v, false),
        // A GIF Reddit transcoded (imgur .gifv and the like): a silent loop.
        (None, Some(v)) => (v, true),
        (None, None) => return None,
    };

    let fallback = json_str(video, "fallback_url").and_then(media_url);
    let vid = fallback
        .as_deref()
        .or_else(|| json_str(video, "hls_url"))
        .and_then(vreddit_id);
    let hls_url = vid
        .as_deref()
        .map(stable_hls_url)
        .or_else(|| json_str(video, "hls_url").and_then(media_url));
    let has_audio = video.get("has_audio").and_then(|v| v.as_bool()).unwrap_or(false);
    let width = json_dim(video, "width");
    let height = json_dim(video, "height");

    // `fallback_url` is one DASH video track. With no audio track that is the
    // whole video and a `<video src>` plays it; with one, it would play
    // silent, so the playlist — which carries both — is the file instead.
    let sources: Vec<VideoSource> = match (&fallback, has_audio && !is_clip) {
        (Some(url), false) => vec![VideoSource { url: url.clone(), width, height }],
        _ => Vec::new(),
    };
    let url = sources
        .last()
        .map(|s| s.url.clone())
        .or_else(|| hls_url.clone())?;

    Some(RedditMedia {
        kind: if is_clip || video.get("is_gif").and_then(|v| v.as_bool()) == Some(true) {
            MediaKind::Clip
        } else {
            MediaKind::Video
        },
        url,
        width,
        height,
        thumbnail_url: json_preview(post).map(|(u, _, _)| u),
        hls_url,
        sources,
        duration_secs: video.get("duration").and_then(|v| v.as_f64()),
        caption: None,
    })
}

fn json_gallery(post: &serde_json::Value) -> Vec<RedditMedia> {
    let Some(items) = post.pointer("/gallery_data/items").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    let metadata = post.get("media_metadata");
    let mut media = Vec::new();
    for item in items {
        let Some(mid) = json_str(item, "media_id") else { continue };
        let Some(meta) = metadata.and_then(|m| m.get(mid)) else { continue };
        if json_str(meta, "status") != Some("valid") {
            continue;
        }
        let Some(s) = meta.get("s") else { continue };
        let thumbnail_url = meta
            .get("p")
            .and_then(|p| p.as_array())
            .and_then(|p| {
                p.iter()
                    .rfind(|r| json_dim(r, "x").is_some_and(|x| x <= 640))
            })
            .and_then(|r| json_str(r, "u"))
            .and_then(media_url);
        let (kind, url) = match json_str(meta, "e") {
            Some("AnimatedImage") => match (json_str(s, "mp4"), json_str(s, "gif")) {
                (Some(mp4), _) => (MediaKind::Clip, media_url(mp4)),
                (None, Some(gif)) => (MediaKind::Gif, media_url(gif)),
                _ => continue,
            },
            Some("Image") => {
                let url = json_str(s, "u").and_then(media_url);
                (url.as_deref().map(image_kind).unwrap_or(MediaKind::Image), url)
            }
            _ => continue,
        };
        let Some(url) = url else { continue };
        let caption = json_str(item, "caption").map(|c| bounded(c.to_string(), 300));
        media.push(RedditMedia {
            kind,
            url,
            width: json_dim(s, "x"),
            height: json_dim(s, "y"),
            thumbnail_url,
            hls_url: None,
            sources: Vec::new(),
            duration_secs: None,
            caption,
        });
    }
    media
}

fn json_image(post: &serde_json::Value) -> Option<RedditMedia> {
    let direct = json_str(post, "url_overridden_by_dest")
        .or_else(|| json_str(post, "url"))
        .and_then(media_url)
        .filter(|u| host_of(u).as_deref() == Some("i.redd.it"));
    let preview = json_preview(post);
    let is_image_post = json_str(post, "post_hint") == Some("image");
    let (url, width, height) = match (direct, &preview) {
        (Some(url), Some((_, w, h))) => (url, *w, *h),
        (Some(url), None) => (url, None, None),
        (None, Some((url, w, h))) if is_image_post => (url.clone(), *w, *h),
        _ => return None,
    };
    Some(RedditMedia {
        kind: image_kind(&url),
        thumbnail_url: preview.map(|(u, _, _)| u).filter(|u| *u != url),
        url,
        width,
        height,
        hls_url: None,
        sources: Vec::new(),
        duration_secs: None,
        caption: None,
    })
}

/// Read a `/comments/{id}.json` listing for post `id`.
fn parse_listing(body: &str, id: &str) -> Result<RedditPost, EmbedError> {
    let root: serde_json::Value = serde_json::from_str(body)
        .map_err(|_| EmbedError::Unrecognised(format!("not json ({})", bounded(body.chars().take(80).collect(), 80))))?;
    let post = root
        .pointer("/0/data/children/0/data")
        .ok_or_else(|| EmbedError::Unrecognised("listing has no post".into()))?;
    if json_str(post, "id") != Some(id) {
        return Err(EmbedError::Unrecognised(format!(
            "listing describes {:?}, not {id}",
            json_str(post, "id")
        )));
    }

    // A crosspost carries its media on the original; the title, subreddit and
    // author shown are still the crosspost's own.
    let media_post = post
        .pointer("/crosspost_parent_list/0")
        .filter(|p| p.is_object())
        .unwrap_or(post);

    let mut media: Vec<RedditMedia> = json_video(media_post).into_iter().collect();
    if media.is_empty() {
        media = json_gallery(media_post);
    }
    if media.is_empty() {
        media.extend(json_image(media_post));
    }
    if media.is_empty() {
        if post.get("removed_by_category").is_some_and(|v| !v.is_null()) {
            return Err(EmbedError::Gone("post removed".into()));
        }
        return Err(EmbedError::NoMedia(
            json_str(post, "post_hint").unwrap_or("text or link").to_string() + " post",
        ));
    }

    let subreddit = json_str(post, "subreddit")
        .filter(|s| valid_subreddit(s))
        .map(str::to_string)
        .ok_or_else(|| EmbedError::Unrecognised("listing has no subreddit".into()))?;
    let author = json_str(post, "author")
        .filter(|a| valid_username(a) && *a != "[deleted]")
        .map(str::to_string);
    let expires_at = media
        .iter()
        .flat_map(|m| std::iter::once(&m.url).chain(m.sources.iter().map(|s| &s.url)))
        .filter_map(|u| signed_expiry_ms(u))
        .min();

    Ok(RedditPost {
        id: id.to_string(),
        permalink: permalink(&subreddit, id),
        title: json_str(post, "title").map(|t| bounded(decode_entities(t), 300)),
        author,
        score: post.get("score").and_then(|v| v.as_i64()),
        comment_count: post.get("num_comments").and_then(|v| v.as_i64()),
        nsfw: post.get("over_18").and_then(|v| v.as_bool()).unwrap_or(false),
        created_at: post
            .get("created_utc")
            .and_then(|v| v.as_f64())
            .map(|s| (s * 1000.0) as i64),
        subreddit,
        media,
        expires_at,
        source: PostSource::Json,
    })
}

/* -------------------------------------------------------------------------- */
/* The command                                                                 */
/* -------------------------------------------------------------------------- */

async fn from_embed(id: &str, subreddit: Option<&str>) -> Result<RedditPost, EmbedError> {
    let sub = subreddit.unwrap_or(PLACEHOLDER_SUBREDDIT);
    let url = format!("{EMBED_ORIGIN}/r/{sub}/comments/{id}/");
    let answer = get(&url, "text/html", EMBED_MAX_BYTES)
        .await
        .map_err(EmbedError::Unrecognised)?;
    if !answer.status.is_success() {
        return Err(EmbedError::Unrecognised(format!("embed HTTP {}", answer.status)));
    }
    parse_embed(&answer.body, id)
}

async fn from_json(id: &str) -> Result<RedditPost, EmbedError> {
    let url = format!("{WWW_ORIGIN}/comments/{id}.json?raw_json=1&limit=1&depth=1");
    let answer = get(&url, "application/json", JSON_MAX_BYTES)
        .await
        .map_err(EmbedError::Unrecognised)?;
    if answer.status == reqwest::StatusCode::NOT_FOUND {
        return Err(EmbedError::Gone("json HTTP 404".into()));
    }
    if !answer.status.is_success() {
        // A 403 here is the address block described at the top of this file,
        // not something wrong with the post.
        return Err(EmbedError::Unrecognised(format!("json HTTP {}", answer.status)));
    }
    parse_listing(&answer.body, id)
}

/// Resolve a Reddit post to its media.
///
/// Errors are strings, prefixed `gone:` (deleted, removed, no such post),
/// `nomedia:` (a text or link post) or `invalid:` (a target that failed
/// validation) when asking again cannot help; anything else is transient.
#[tauri::command]
pub async fn fetch_reddit_post(target: RedditTarget) -> Result<RedditPost, String> {
    let (subreddit, id) = match target {
        RedditTarget::Post { id, subreddit } => {
            let id = id.to_ascii_lowercase();
            if !valid_post_id(&id) {
                return Err(format!("{ERR_INVALID} post id"));
            }
            if subreddit.as_deref().is_some_and(|s| !valid_subreddit(s)) {
                return Err(format!("{ERR_INVALID} subreddit"));
            }
            (subreddit, id)
        }
        RedditTarget::Share { subreddit, token } => {
            if !valid_subreddit(&subreddit) || !valid_share_token(&token) {
                return Err(format!("{ERR_INVALID} share link"));
            }
            resolve_share(&subreddit, &token).await?
        }
    };

    let embed_err = match from_embed(&id, subreddit.as_deref()).await {
        Ok(post) => return Ok(post),
        Err(EmbedError::Gone(m)) => return Err(format!("{ERR_GONE} {m}")),
        Err(EmbedError::NoMedia(m)) => return Err(format!("{ERR_NO_MEDIA} {m}")),
        Err(EmbedError::Unrecognised(m)) => m,
    };
    eprintln!("[reddit] embed route failed for {id}: {embed_err}; trying .json");

    match from_json(&id).await {
        Ok(post) => Ok(post),
        Err(EmbedError::Gone(m)) => Err(format!("{ERR_GONE} {m}")),
        Err(EmbedError::NoMedia(m)) => Err(format!("{ERR_NO_MEDIA} {m}")),
        Err(EmbedError::Unrecognised(m)) => Err(format!("embed: {embed_err}; json: {m}")),
    }
}

/* -------------------------------------------------------------------------- */
/* Tests                                                                       */
/* -------------------------------------------------------------------------- */

// Fixtures are real `embed.reddit.com` pages captured 03.10.2026 with
// <style>/<script>/<svg>/<template> blocks stripped and the post markup left
// byte-for-byte. The `listing_*.json` files are real post objects (from the
// Arctic Shift archive, which stores Reddit's own t3 JSON) in the two-listing
// envelope `/comments/{id}.json` returns — the live endpoint itself is
// address-blocked from where they were captured.
#[cfg(test)]
mod tests {
    use super::*;

    const VIDEO_SILENT: &str = include_str!("../tests/fixtures/reddit/video_silent.html");
    const VIDEO_AUDIO: &str = include_str!("../tests/fixtures/reddit/video_audio.html");
    const GALLERY: &str = include_str!("../tests/fixtures/reddit/gallery.html");
    const GALLERY_OTHER_SUB: &str = include_str!("../tests/fixtures/reddit/gallery_other_sub.html");
    const IMAGE: &str = include_str!("../tests/fixtures/reddit/image.html");
    const IMAGE_NSFW: &str = include_str!("../tests/fixtures/reddit/image_nsfw.html");
    const DELETED: &str = include_str!("../tests/fixtures/reddit/deleted.html");
    const MISSING: &str = include_str!("../tests/fixtures/reddit/missing.html");
    const UNSUPPORTED_ALL: &str = include_str!("../tests/fixtures/reddit/unsupported_all.html");
    const LISTING_VIDEO: &str = include_str!("../tests/fixtures/reddit/listing_video.json");
    const LISTING_GALLERY: &str = include_str!("../tests/fixtures/reddit/listing_gallery.json");
    const LISTING_IMAGE: &str = include_str!("../tests/fixtures/reddit/listing_image.json");

    #[test]
    fn embed_video_with_audio_prefers_the_largest_muxed_mp4() {
        let post = parse_embed(VIDEO_AUDIO, "1wwkjn3").unwrap();
        assert_eq!(post.subreddit, "funny");
        assert_eq!(post.title.as_deref(), Some("Funny waterfall video"));
        assert_eq!(post.source, PostSource::Embed);
        assert_eq!(post.media.len(), 1);
        let video = &post.media[0];
        assert_eq!(video.kind, MediaKind::Video);
        assert!(video.url.starts_with("https://packaged-media.redd.it/oho9co2hi8th1/pb/m2-res_640p.mp4?"));
        // Decoded: the signature only matches with real `&` separators.
        assert!(!video.url.contains("&amp;"));
        assert_eq!((video.width, video.height), (Some(360), Some(640)));
        assert_eq!(video.sources.len(), 3);
        assert!(video.sources.windows(2).all(|w| w[0].height <= w[1].height));
        assert_eq!(video.hls_url.as_deref(), Some("https://v.redd.it/oho9co2hi8th1/HLSPlaylist.m3u8"));
        assert!(video.thumbnail_url.as_deref().unwrap().starts_with("https://external-preview.redd.it/"));
        assert_eq!(video.duration_secs, Some(10.0));
        assert_eq!(post.expires_at, Some(1_791_043_200_000));
    }

    #[test]
    fn embed_video_reads_title_author_score_and_comments() {
        let post = parse_embed(VIDEO_SILENT, "1ww1h11").unwrap();
        assert_eq!(post.subreddit, "whatisit");
        assert_eq!(post.title.as_deref(), Some("What was this thing I captured on film?"));
        assert_eq!(post.author.as_deref(), Some("deezheavenz"));
        assert_eq!(post.score, Some(7678));
        assert_eq!(post.comment_count, Some(1630));
        assert!(!post.nsfw);
        assert_eq!(post.created_at, Some(1_790_966_109_901));
        assert_eq!(post.permalink, "https://www.reddit.com/r/whatisit/comments/1ww1h11/");
        assert_eq!(post.media[0].kind, MediaKind::Video);
    }

    #[test]
    fn embed_gallery_lists_every_original_in_carousel_order() {
        let post = parse_embed(GALLERY, "1wwkabj").unwrap();
        let urls: Vec<&str> = post.media.iter().map(|m| m.url.as_str()).collect();
        // The order `gallery_data.items` gives for the same post.
        assert_eq!(
            urls,
            [
                "https://i.redd.it/86vitc9nf8th1.jpg",
                "https://i.redd.it/u85xvyxnf8th1.jpg",
                "https://i.redd.it/65s7kb6of8th1.jpg",
                "https://i.redd.it/r3lwj8bof8th1.jpg",
                "https://i.redd.it/a4jrpekof8th1.jpg",
                "https://i.redd.it/tc8v5hoof8th1.jpg",
            ]
        );
        assert!(post.media.iter().all(|m| m.kind == MediaKind::Image));
        assert!(post.media.iter().all(|m| m
            .thumbnail_url
            .as_deref()
            .is_some_and(|t| t.starts_with("https://preview.redd.it/"))));
        assert_eq!(post.title.as_deref(), Some("[OC] Night photos"));
        assert_eq!(post.expires_at, None);
    }

    #[test]
    fn embed_image_uses_the_original_file() {
        let post = parse_embed(IMAGE, "1wwk0gh").unwrap();
        assert_eq!(post.media.len(), 1);
        assert_eq!(post.media[0].url, "https://i.redd.it/vtug8senc8th1.jpeg");
        assert_eq!(post.media[0].kind, MediaKind::Image);
        assert!(post.media[0].thumbnail_url.as_deref().unwrap().starts_with("https://preview.redd.it/"));
        assert_eq!(post.score, Some(14));
    }

    #[test]
    fn embed_nsfw_image_is_served_and_flagged() {
        let post = parse_embed(IMAGE_NSFW, "1wwkaw6").unwrap();
        assert!(post.nsfw);
        assert_eq!(post.media[0].url, "https://i.redd.it/8brag80vf8th1.jpeg");
        // Entity-decoded: the raw markup has `doesn&#39;t`.
        assert!(post.title.as_deref().unwrap().starts_with("Tatsumaki doesn't want"));
    }

    #[test]
    fn embed_deleted_post_is_gone() {
        assert!(matches!(parse_embed(DELETED, "1wwk7bp"), Err(EmbedError::Gone(_))));
    }

    #[test]
    fn embed_missing_post_falls_through_to_json() {
        assert!(matches!(parse_embed(MISSING, "zzzzzzz"), Err(EmbedError::Unrecognised(_))));
    }

    #[test]
    fn embed_r_all_is_not_an_answer() {
        assert!(matches!(
            parse_embed(UNSUPPORTED_ALL, "1wwkabj"),
            Err(EmbedError::Unrecognised(_))
        ));
        // The screenview echoes the URL's subreddit, not the post's — which is
        // why the subreddit is read from the permalink instead.
        let view = screenview(UNSUPPORTED_ALL).unwrap();
        assert_eq!(view.subreddit.as_deref(), Some("all"));
    }

    #[test]
    fn embed_under_another_subreddit_reports_the_real_one() {
        // Fetched as `r/wrongsub/comments/1wwkabj` — a real r/pics post.
        assert_eq!(screenview(GALLERY_OTHER_SUB).unwrap().subreddit.as_deref(), Some("wrongsub"));
        let post = parse_embed(GALLERY_OTHER_SUB, "1wwkabj").unwrap();
        assert_eq!(post.subreddit, "pics");
        assert_eq!(post.permalink, "https://www.reddit.com/r/pics/comments/1wwkabj/");
        assert_eq!(post.media.len(), 6);
    }

    #[test]
    fn embed_for_a_different_post_is_rejected() {
        assert!(matches!(parse_embed(IMAGE, "1wwkabj"), Err(EmbedError::Unrecognised(_))));
    }

    #[test]
    fn listing_video_with_audio_plays_the_playlist() {
        let post = parse_listing(LISTING_VIDEO, "1wwkjn3").unwrap();
        assert_eq!(post.source, PostSource::Json);
        let video = &post.media[0];
        assert_eq!(video.kind, MediaKind::Video);
        // `fallback_url` is video-only; with an audio track it would play mute.
        assert!(video.sources.is_empty());
        assert_eq!(video.url, "https://v.redd.it/oho9co2hi8th1/HLSPlaylist.m3u8");
        assert_eq!(video.hls_url.as_deref(), Some(video.url.as_str()));
        assert_eq!((video.width, video.height), (Some(360), Some(640)));
        assert_eq!(post.subreddit, "funny");
        assert_eq!(post.title.as_deref(), Some("Funny waterfall video"));
    }

    #[test]
    fn listing_video_without_audio_plays_the_fallback_mp4() {
        let mut root: serde_json::Value = serde_json::from_str(LISTING_VIDEO).unwrap();
        let post = root.pointer_mut("/0/data/children/0/data").unwrap();
        for key in ["media", "secure_media"] {
            if let Some(v) = post.pointer_mut(&format!("/{key}/reddit_video/has_audio")) {
                *v = serde_json::Value::Bool(false);
            }
        }
        let post = parse_listing(&root.to_string(), "1wwkjn3").unwrap();
        let video = &post.media[0];
        assert_eq!(video.sources.len(), 1);
        assert!(video.url.starts_with("https://v.redd.it/oho9co2hi8th1/CMAF_"));
        assert_eq!(video.hls_url.as_deref(), Some("https://v.redd.it/oho9co2hi8th1/HLSPlaylist.m3u8"));
    }

    #[test]
    fn listing_gallery_matches_the_embed_route() {
        let from_json = parse_listing(LISTING_GALLERY, "1wwkabj").unwrap();
        let from_embed = parse_embed(GALLERY, "1wwkabj").unwrap();
        let json_ids: Vec<String> = from_json
            .media
            .iter()
            .map(|m| m.url.rsplit('/').next().unwrap().split('.').next().unwrap().to_string())
            .collect();
        let embed_ids: Vec<String> = from_embed
            .media
            .iter()
            .map(|m| m.url.rsplit('/').next().unwrap().split('.').next().unwrap().to_string())
            .collect();
        assert_eq!(json_ids, embed_ids);
        assert!(from_json.media.iter().all(|m| m.width.is_some() && m.height.is_some()));
    }

    #[test]
    fn listing_image_reads_dimensions() {
        let post = parse_listing(LISTING_IMAGE, "1wwk0gh").unwrap();
        assert_eq!(post.media[0].url, "https://i.redd.it/vtug8senc8th1.jpeg");
        assert!(post.media[0].width.is_some());
    }

    #[test]
    fn listing_for_a_different_post_is_rejected() {
        assert!(matches!(parse_listing(LISTING_IMAGE, "1wwkabj"), Err(EmbedError::Unrecognised(_))));
    }

    #[test]
    fn listing_blocked_html_is_unrecognised() {
        let blocked = "<body class=theme-beta><div>You've been blocked by network security.</div>";
        assert!(matches!(parse_listing(blocked, "1ww1h11"), Err(EmbedError::Unrecognised(_))));
    }

    #[test]
    fn media_urls_must_be_https_on_redd_it() {
        assert!(media_url("https://i.redd.it/a.jpg").is_some());
        assert!(media_url("https://packaged-media.redd.it/x/pb/m2.mp4?a=1&amp;b=2")
            .is_some_and(|u| u.ends_with("?a=1&b=2")));
        assert!(media_url("http://i.redd.it/a.jpg").is_none());
        assert!(media_url("https://i.redd.it.evil.example/a.jpg").is_none());
        assert!(media_url("https://evilredd.it/a.jpg").is_none());
        assert!(media_url("javascript:alert(1)").is_none());
        assert!(media_url("https://reddit.com/a.jpg").is_none());
    }

    #[test]
    fn post_paths_parse() {
        assert_eq!(
            parse_post_path("/r/whatisit/comments/1ww1h11/what_was_this/"),
            Some((Some("whatisit".into()), "1ww1h11".into()))
        );
        assert_eq!(parse_post_path("/comments/1ww1h11"), Some((None, "1ww1h11".into())));
        assert_eq!(
            parse_post_path("/user/someone/comments/abc123/x/"),
            Some((Some("u_someone".into()), "abc123".into()))
        );
        assert_eq!(parse_post_path("/r/whatisit/"), None);
        assert_eq!(parse_post_path("/r/whatisit/comments/../etc"), None);
    }

    #[test]
    fn targets_validate() {
        assert!(valid_post_id("1ww1h11"));
        assert!(!valid_post_id("1WW1H11"));
        assert!(!valid_post_id("1ww1h11/../x"));
        assert!(!valid_post_id(""));
        assert!(valid_subreddit("whatisit"));
        assert!(valid_subreddit("u_some-one"));
        assert!(!valid_subreddit("what/isit"));
        assert!(!valid_subreddit("a?b"));
        assert!(valid_share_token("AbC123xyz"));
        assert!(!valid_share_token("abc/def"));
    }

    #[test]
    fn gallery_original_strips_the_slug() {
        assert_eq!(
            gallery_original("https://preview.redd.it/night-photos-v0-86vitc9nf8th1.jpg?width=640"),
            Some(("86vitc9nf8th1".into(), "https://i.redd.it/86vitc9nf8th1.jpg".into()))
        );
        assert_eq!(
            gallery_original("https://preview.redd.it/a4jrpekof8th1.png?width=108"),
            Some(("a4jrpekof8th1".into(), "https://i.redd.it/a4jrpekof8th1.png".into()))
        );
    }

    /// Hits the live site. `cargo test --lib reddit -- --ignored --nocapture`.
    #[test]
    #[ignore = "network"]
    fn live_posts_resolve() {
        let run = |target: RedditTarget| tauri::async_runtime::block_on(fetch_reddit_post(target));
        let post = |id: &str, sub: Option<&str>| RedditTarget::Post {
            id: id.into(),
            subreddit: sub.map(str::to_string),
        };

        let video = run(post("1ww1h11", Some("whatisit"))).unwrap();
        eprintln!("video: {video:?}");
        assert_eq!(video.media[0].kind, MediaKind::Video);
        assert!(!video.media[0].sources.is_empty());

        // No subreddit in the link (`redd.it/{id}`): the placeholder route.
        let gallery = run(post("1wwkabj", None)).unwrap();
        eprintln!("gallery: {} items from {:?}", gallery.media.len(), gallery.source);
        assert_eq!(gallery.subreddit, "pics");
        assert_eq!(gallery.media.len(), 6);

        let nsfw = run(post("1wwkaw6", Some("rule34"))).unwrap();
        assert!(nsfw.nsfw);

        let deleted = run(post("1wwk7bp", Some("rule34"))).unwrap_err();
        eprintln!("deleted: {deleted}");
        assert!(deleted.starts_with(ERR_GONE));

        let invalid = run(post("../x", None)).unwrap_err();
        assert!(invalid.starts_with(ERR_INVALID));
    }

    #[test]
    fn tag_end_skips_quoted_angle_brackets() {
        let html = r#"<a title="x > y" href='z'>text"#;
        assert_eq!(tag_end(html, 0), Some(html.find("'>").unwrap() + 1));
    }
}
