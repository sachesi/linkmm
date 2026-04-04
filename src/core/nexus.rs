use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

// ── Public types ──────────────────────────────────────────────────────────────

// These types and methods are part of the Nexus API client which will be used
// in future download functionality. Suppress dead-code lints until then.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct NexusUser {
    pub user_id: u64,
    pub username: String,
    pub email: String,
    pub is_premium: bool,
    pub is_supporter: bool,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct NexusMod {
    pub mod_id: u64,
    pub name: String,
    pub summary: Option<String>,
    pub version: Option<String>,
    pub author: Option<String>,
}

/// Richer mod info returned by list endpoints (trending, latest, etc.).
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct NexusModInfo {
    pub mod_id: u64,
    pub name: String,
    pub summary: Option<String>,
    pub version: Option<String>,
    pub author: Option<String>,
    pub endorsement_count: u64,
    pub picture_url: Option<String>,
}

/// A downloadable file belonging to a Nexus mod.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct NexusModFile {
    pub file_id: u64,
    pub name: String,
    pub version: Option<String>,
    pub category_name: String,
    pub is_primary: bool,
    pub size_kb: u64,
    pub file_name: String,
}

/// Non-sensitive counters for Nexus API behavior.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NexusTelemetry {
    pub api_requests_saved_by_cache: u64,
}

// ── Private deserialization types ─────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct NexusUserResponse {
    user_id: u64,
    name: String,
    email: String,
    is_premium: bool,
    is_supporter: bool,
}

#[derive(Debug, Deserialize)]
struct NexusModResponse {
    mod_id: u64,
    name: String,
    summary: Option<String>,
    version: Option<String>,
    author: Option<String>,
}

#[derive(Debug, Deserialize)]
struct NexusModListItem {
    mod_id: u64,
    name: String,
    summary: Option<String>,
    version: Option<String>,
    author: Option<String>,
    #[serde(default)]
    endorsement_count: u64,
    picture_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct NexusModFileResponse {
    file_id: u64,
    name: String,
    version: Option<String>,
    category_name: Option<String>,
    #[serde(default)]
    is_primary: bool,
    #[serde(default)]
    size_kb: u64,
    file_name: String,
}

#[derive(Debug, Deserialize)]
struct NexusFilesResponse {
    files: Vec<NexusModFileResponse>,
}

#[derive(Debug, Deserialize)]
struct NexusDownloadLink {
    #[serde(rename = "URI")]
    uri: String,
    short_name: String,
}

#[derive(Debug)]
struct CacheEntry {
    body: String,
    expires_at: Instant,
}

#[derive(Debug)]
struct RequestError {
    message: String,
    transient: bool,
}

// ── Client ────────────────────────────────────────────────────────────────────

pub struct NexusClient {
    pub api_key: String,
    cache_ttl: Duration,
    max_retries: usize,
    backoff_base_ms: u64,
    cache: Mutex<HashMap<String, CacheEntry>>,
    cache_hits: AtomicU64,
}

#[allow(dead_code)]
impl NexusClient {
    pub fn new(api_key: &str) -> Self {
        Self {
            api_key: api_key.to_string(),
            cache_ttl: Duration::from_secs(60),
            max_retries: 2,
            backoff_base_ms: 250,
            cache: Mutex::new(HashMap::new()),
            cache_hits: AtomicU64::new(0),
        }
    }

    pub fn telemetry(&self) -> NexusTelemetry {
        NexusTelemetry {
            api_requests_saved_by_cache: self.cache_hits.load(Ordering::Relaxed),
        }
    }

    fn get_json<T: for<'de> Deserialize<'de>>(&self, url: &str) -> Result<T, String> {
        let body = self.get_body_with_retry(url, None)?;
        serde_json::from_str(&body).map_err(|e| format!("Failed to parse response: {e}"))
    }

    fn get_json_cached<T: for<'de> Deserialize<'de>>(&self, url: &str) -> Result<T, String> {
        let body = self.get_cached_or_fetch(url)?;
        serde_json::from_str(&body).map_err(|e| format!("Failed to parse response: {e}"))
    }

    fn get_cached_or_fetch(&self, url: &str) -> Result<String, String> {
        if let Ok(cache) = self.cache.lock()
            && let Some(entry) = cache.get(url)
            && entry.expires_at > Instant::now()
        {
            self.cache_hits.fetch_add(1, Ordering::Relaxed);
            return Ok(entry.body.clone());
        }

        let body = self.get_body_with_retry(url, None)?;

        if let Ok(mut cache) = self.cache.lock() {
            cache.insert(
                url.to_string(),
                CacheEntry {
                    body: body.clone(),
                    expires_at: Instant::now() + self.cache_ttl,
                },
            );
        }

        Ok(body)
    }

    fn get_body_with_retry(
        &self,
        url: &str,
        query: Option<&[(&str, &str)]>,
    ) -> Result<String, String> {
        let mut last_err = String::new();

        for attempt in 0..=self.max_retries {
            match self.perform_get(url, query) {
                Ok(body) => return Ok(body),
                Err(err) => {
                    last_err = err.message;
                    if !err.transient || attempt == self.max_retries {
                        break;
                    }
                    let sleep_ms = self.backoff_base_ms * (1_u64 << attempt);
                    std::thread::sleep(Duration::from_millis(sleep_ms));
                }
            }
        }

        Err(last_err)
    }

    fn perform_get(
        &self,
        url: &str,
        query: Option<&[(&str, &str)]>,
    ) -> Result<String, RequestError> {
        let mut req = ureq::get(url)
            .set("apikey", &self.api_key)
            .set("User-Agent", "Linkmm/0.1.0");

        if let Some(query) = query {
            for (key, value) in query {
                req = req.query(key, value);
            }
        }

        req.call()
            .and_then(|resp| resp.into_string().map_err(ureq::Error::from))
            .map_err(|err| match err {
                ureq::Error::Status(code, resp) => {
                    let body = resp.into_string().unwrap_or_default();
                    RequestError {
                        message: format!("Request failed with status {code}: {body}"),
                        transient: code == 408 || code == 429 || (500..=599).contains(&code),
                    }
                }
                ureq::Error::Transport(t) => {
                    let msg = t.to_string();
                    let lower = msg.to_lowercase();
                    let transient = lower.contains("timed out")
                        || lower.contains("timeout")
                        || lower.contains("tempor")
                        || lower.contains("connection reset")
                        || lower.contains("connection refused")
                        || lower.contains("dns");
                    RequestError {
                        message: format!("Request failed: {msg}"),
                        transient,
                    }
                }
            })
    }

    pub fn validate(&self) -> Result<NexusUser, String> {
        let data: NexusUserResponse =
            self.get_json("https://api.nexusmods.com/v1/users/validate.json")?;
        Ok(NexusUser {
            user_id: data.user_id,
            username: data.name,
            email: data.email,
            is_premium: data.is_premium,
            is_supporter: data.is_supporter,
        })
    }

    pub fn get_mod(&self, game_domain: &str, mod_id: u32) -> Result<NexusMod, String> {
        let url = format!("https://api.nexusmods.com/v1/games/{game_domain}/mods/{mod_id}.json");
        let data: NexusModResponse = self.get_json_cached(&url)?;
        Ok(NexusMod {
            mod_id: data.mod_id,
            name: data.name,
            summary: data.summary,
            version: data.version,
            author: data.author,
        })
    }

    /// Fetch trending mods for the given game domain.
    pub fn list_trending_mods(&self, game_domain: &str) -> Result<Vec<NexusModInfo>, String> {
        self.fetch_mod_list(&format!(
            "https://api.nexusmods.com/v1/games/{game_domain}/mods/trending.json"
        ))
    }

    /// Fetch the ten most recently added mods for the given game domain.
    pub fn list_latest_added_mods(&self, game_domain: &str) -> Result<Vec<NexusModInfo>, String> {
        self.fetch_mod_list(&format!(
            "https://api.nexusmods.com/v1/games/{game_domain}/mods/latest_added.json"
        ))
    }

    fn fetch_mod_list(&self, url: &str) -> Result<Vec<NexusModInfo>, String> {
        let data: Vec<NexusModListItem> = self.get_json_cached(url)?;
        Ok(data
            .into_iter()
            .map(|m| NexusModInfo {
                mod_id: m.mod_id,
                name: m.name,
                summary: m.summary,
                version: m.version,
                author: m.author,
                endorsement_count: m.endorsement_count,
                picture_url: m.picture_url,
            })
            .collect())
    }

    /// List files available for the given mod.
    pub fn get_mod_files(
        &self,
        game_domain: &str,
        mod_id: u32,
    ) -> Result<Vec<NexusModFile>, String> {
        let url =
            format!("https://api.nexusmods.com/v1/games/{game_domain}/mods/{mod_id}/files.json");
        let data: NexusFilesResponse = self.get_json_cached(&url)?;
        Ok(data
            .files
            .into_iter()
            .map(|f| NexusModFile {
                file_id: f.file_id,
                name: f.name,
                version: f.version,
                category_name: f.category_name.unwrap_or_else(|| "Unknown".into()),
                is_primary: f.is_primary,
                size_kb: f.size_kb,
                file_name: f.file_name,
            })
            .collect())
    }

    /// Get CDN download links for a specific file (requires Nexus Premium).
    ///
    /// Returns a list of `(cdn_name, download_url)` pairs.
    pub fn get_download_links(
        &self,
        game_domain: &str,
        mod_id: u32,
        file_id: u64,
    ) -> Result<Vec<(String, String)>, String> {
        let url = format!(
            "https://api.nexusmods.com/v1/games/{game_domain}/mods/{mod_id}/files/{file_id}/download_link.json"
        );
        let data: Vec<NexusDownloadLink> = self.get_json(&url)?;
        Ok(data.into_iter().map(|l| (l.short_name, l.uri)).collect())
    }

    /// Get CDN download links using NXM key/expires parameters.
    ///
    /// This is used when the app receives an `nxm://` URL from the browser.
    /// Any Nexus account can use this endpoint when valid key/expires are
    /// provided.
    pub fn get_download_links_nxm(
        &self,
        game_domain: &str,
        mod_id: u32,
        file_id: u64,
        key: &str,
        expires: &str,
    ) -> Result<Vec<(String, String)>, String> {
        let url = format!(
            "https://api.nexusmods.com/v1/games/{game_domain}/mods/{mod_id}/files/{file_id}/download_link.json"
        );
        let query = [("key", key), ("expires", expires)];
        let body = self.get_body_with_retry(&url, Some(&query))?;
        let data: Vec<NexusDownloadLink> =
            serde_json::from_str(&body).map_err(|e| format!("Failed to parse response: {e}"))?;
        Ok(data.into_iter().map(|l| (l.short_name, l.uri)).collect())
    }

    /// Return the public Nexus Mods page URL for the given mod.
    pub fn mod_page_url(game_domain: &str, mod_id: u64) -> String {
        format!("https://www.nexusmods.com/{game_domain}/mods/{mod_id}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telemetry_defaults_to_zero() {
        let client = NexusClient::new("abc");
        assert_eq!(
            client.telemetry(),
            NexusTelemetry {
                api_requests_saved_by_cache: 0
            }
        );
    }

    #[test]
    fn mod_page_url_is_stable() {
        assert_eq!(
            NexusClient::mod_page_url("skyrimspecialedition", 123),
            "https://www.nexusmods.com/skyrimspecialedition/mods/123"
        );
    }

    #[test]
    fn cache_can_store_and_hit_without_network() {
        let client = NexusClient::new("abc");
        {
            let mut cache = client.cache.lock().unwrap();
            cache.insert(
                "https://example.test/item".to_string(),
                CacheEntry {
                    body: "{\"mod_id\":1,\"name\":\"X\",\"summary\":null,\"version\":null,\"author\":null}".to_string(),
                    expires_at: Instant::now() + Duration::from_secs(10),
                },
            );
        }

        let parsed: NexusModResponse = client
            .get_json_cached("https://example.test/item")
            .expect("should parse cached json");
        assert_eq!(parsed.mod_id, 1);
        assert_eq!(client.telemetry().api_requests_saved_by_cache, 1);
    }
}
