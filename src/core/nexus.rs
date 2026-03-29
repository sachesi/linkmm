use serde::Deserialize;

// ── Public types ──────────────────────────────────────────────────────────────

// These types and methods are part of the Nexus API client which will be used
// in future download functionality.  Suppress dead-code lints until then.
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

// ── Client ────────────────────────────────────────────────────────────────────

pub struct NexusClient {
    pub api_key: String,
}

#[allow(dead_code)]
impl NexusClient {
    pub fn new(api_key: &str) -> Self {
        Self {
            api_key: api_key.to_string(),
        }
    }

    fn get(&self, url: &str) -> Result<ureq::Response, String> {
        ureq::get(url)
            .set("apikey", &self.api_key)
            .set("User-Agent", "Linkmm/0.1.0")
            .call()
            .map_err(|e| format!("Request failed: {e}"))
    }

    pub fn validate(&self) -> Result<NexusUser, String> {
        let response = self.get("https://api.nexusmods.com/v1/users/validate.json")?;
        let data: NexusUserResponse = response
            .into_json()
            .map_err(|e| format!("Failed to parse response: {e}"))?;
        Ok(NexusUser {
            user_id: data.user_id,
            username: data.name,
            email: data.email,
            is_premium: data.is_premium,
            is_supporter: data.is_supporter,
        })
    }

    pub fn get_mod(&self, game_domain: &str, mod_id: u32) -> Result<NexusMod, String> {
        let url = format!(
            "https://api.nexusmods.com/v1/games/{game_domain}/mods/{mod_id}.json"
        );
        let response = self.get(&url)?;
        let data: NexusModResponse = response
            .into_json()
            .map_err(|e| format!("Failed to parse response: {e}"))?;
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
        let response = self.get(url)?;
        let data: Vec<NexusModListItem> = response
            .into_json()
            .map_err(|e| format!("Failed to parse response: {e}"))?;
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
        let url = format!(
            "https://api.nexusmods.com/v1/games/{game_domain}/mods/{mod_id}/files.json"
        );
        let response = self.get(&url)?;
        let data: NexusFilesResponse = response
            .into_json()
            .map_err(|e| format!("Failed to parse response: {e}"))?;
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
        let response = self.get(&url)?;
        let data: Vec<NexusDownloadLink> = response
            .into_json()
            .map_err(|e| format!("Failed to parse response: {e}"))?;
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
        // Use ureq query parameters to avoid URL injection
        let response = ureq::get(&url)
            .set("apikey", &self.api_key)
            .set("User-Agent", "Linkmm/0.1.0")
            .query("key", key)
            .query("expires", expires)
            .call()
            .map_err(|e| format!("Request failed: {e}"))?;
        let data: Vec<NexusDownloadLink> = response
            .into_json()
            .map_err(|e| format!("Failed to parse response: {e}"))?;
        Ok(data.into_iter().map(|l| (l.short_name, l.uri)).collect())
    }

    /// Return the public Nexus Mods page URL for the given mod.
    pub fn mod_page_url(game_domain: &str, mod_id: u64) -> String {
        format!("https://www.nexusmods.com/{game_domain}/mods/{mod_id}")
    }
}
