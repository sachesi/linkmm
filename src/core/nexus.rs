use serde::Deserialize;

#[derive(Debug, Clone)]
pub struct NexusUser {
    pub user_id: u64,
    pub username: String,
    pub email: String,
    pub is_premium: bool,
    pub is_supporter: bool,
}

#[derive(Debug, Clone)]
pub struct NexusMod {
    pub mod_id: u64,
    pub name: String,
    pub summary: Option<String>,
    pub version: Option<String>,
    pub author: Option<String>,
}

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

pub struct NexusClient {
    pub api_key: String,
}

impl NexusClient {
    pub fn new(api_key: &str) -> Self {
        Self {
            api_key: api_key.to_string(),
        }
    }

    pub fn validate(&self) -> Result<NexusUser, String> {
        let response = ureq::get("https://api.nexusmods.com/v1/users/validate.json")
            .set("apikey", &self.api_key)
            .set("User-Agent", "Linkmm/0.1.0")
            .call()
            .map_err(|e| format!("Request failed: {e}"))?;

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
        let response = ureq::get(&url)
            .set("apikey", &self.api_key)
            .set("User-Agent", "Linkmm/0.1.0")
            .call()
            .map_err(|e| format!("Request failed: {e}"))?;

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
}
