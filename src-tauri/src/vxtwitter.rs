//! api.vxtwitter.com, asked from the shell instead of the webview.
//!
//! The API sits behind a Cloudflare rule that answers any request carrying a
//! browser User-Agent with a JS challenge (403, "Just a moment..."), which a
//! `fetch()` can never solve. Measured 16.11.2026 against one tweet:
//!
//! | client                           | User-Agent        | answer        |
//! |----------------------------------|-------------------|---------------|
//! | curl (OpenSSL)                   | curl default      | 200 JSON      |
//! | curl (OpenSSL)                   | Chrome / Edge     | 403 challenge |
//! | Python urllib (different TLS)    | `Prinny (+…)`     | 200 JSON      |
//! | Python urllib                    | Chrome            | 403 challenge |
//! | the app's WebView2 `fetch()`     | Edge (forced)     | 403 challenge |
//!
//! The answer follows the User-Agent across two TLS stacks, so a request from
//! here with an honest non-browser UA gets the JSON. The webview cannot set its
//! own UA on `fetch()`, which is why this lives in the shell.
//!
//! The host and path are fixed and the id must be digits, so page script can
//! only choose which tweet is asked for: there is nothing here to steer at an
//! internal address, and redirects are refused outright.

use std::time::Duration;

const API: &str = "https://api.vxtwitter.com/Twitter/status/";
const USER_AGENT: &str = "Prinny (+https://github.com/coffeegrind123/prinny-client)";
/// A tweet with four media items and a quote is a few KiB. Slack, not an allowance.
const MAX_BYTES: usize = 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(10);
/// Tweet ids are u64 snowflakes: at most 20 decimal digits.
const MAX_ID_LEN: usize = 20;

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_ID_LEN && id.bytes().all(|b| b.is_ascii_digit())
}

/// The vxtwitter JSON for one tweet, unmodified, so the frontend reads it with
/// the same code whichever route it came by.
///
/// vxtwitter reports a deleted or private tweet as a 200 HTML page, not an
/// error status, so anything that is not a JSON object is an error here.
#[tauri::command]
pub async fn fetch_vx_tweet(id: String) -> Result<serde_json::Value, String> {
    if !valid_id(&id) {
        return Err("invalid tweet id".to_string());
    }

    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(TIMEOUT)
        .build()
        .map_err(|e| format!("client: {e}"))?;

    let mut resp = client
        .get(format!("{API}{id}"))
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|e| format!("send: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        return Err(format!("HTTP {}", status.as_u16()));
    }

    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| format!("body: {e}"))? {
        buf.extend_from_slice(&chunk);
        if buf.len() > MAX_BYTES {
            return Err(format!("response over {MAX_BYTES} bytes"));
        }
    }

    match serde_json::from_slice::<serde_json::Value>(&buf) {
        Ok(value) if value.is_object() => Ok(value),
        Ok(_) => Err("not a JSON object".to_string()),
        Err(_) => Err("not JSON (deleted, private or unknown tweet)".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::valid_id;

    #[test]
    fn ids() {
        assert!(valid_id("20"));
        assert!(valid_id("2100884296617890168"));
        assert!(valid_id("18446744073709551615"));
        assert!(!valid_id(""));
        assert!(!valid_id("184467440737095516150"));
        assert!(!valid_id("12a"));
        assert!(!valid_id("../1"));
        assert!(!valid_id("1?x=1"));
        assert!(!valid_id(" 1"));
    }
}
