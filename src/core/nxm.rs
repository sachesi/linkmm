/// Parsed NXM protocol URL.
///
/// Format: `nxm://GAME_DOMAIN/mods/MOD_ID/files/FILE_ID?key=KEY&expires=EXPIRES&user_id=USER_ID`
#[derive(Debug, Clone)]
pub struct NxmUrl {
    pub game_domain: String,
    pub mod_id: u64,
    pub file_id: u64,
    pub key: Option<String>,
    pub expires: Option<String>,
}

impl NxmUrl {
    /// Parse an NXM URL string into its components.
    pub fn parse(url: &str) -> Result<Self, String> {
        let stripped = url
            .strip_prefix("nxm://")
            .ok_or_else(|| format!("Not an NXM URL: {url}"))?;

        // Split path and query
        let (path, query) = match stripped.find('?') {
            Some(idx) => (&stripped[..idx], Some(&stripped[idx + 1..])),
            None => (stripped, None),
        };

        // Parse path segments: GAME_DOMAIN/mods/MOD_ID/files/FILE_ID
        let segments: Vec<&str> = path.split('/').collect();
        if segments.len() < 5 {
            return Err(format!("Invalid NXM URL path: {path}"));
        }

        let game_domain = segments[0].to_string();

        if segments[1] != "mods" {
            return Err(format!("Expected 'mods' in NXM URL, got '{}'", segments[1]));
        }
        let mod_id: u64 = segments[2]
            .parse()
            .map_err(|_| format!("Invalid mod ID: {}", segments[2]))?;

        if segments[3] != "files" {
            return Err(format!(
                "Expected 'files' in NXM URL, got '{}'",
                segments[3]
            ));
        }
        let file_id: u64 = segments[4]
            .parse()
            .map_err(|_| format!("Invalid file ID: {}", segments[4]))?;

        // Parse query parameters
        let mut key = None;
        let mut expires = None;

        if let Some(q) = query {
            for param in q.split('&') {
                if let Some((k, v)) = param.split_once('=') {
                    match k {
                        "key" => key = Some(v.to_string()),
                        "expires" => expires = Some(v.to_string()),
                        _ => {} // ignore unknown params (user_id, etc.)
                    }
                }
            }
        }

        Ok(Self {
            game_domain,
            mod_id,
            file_id,
            key,
            expires,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_full_nxm_url() {
        let url = "nxm://skyrimspecialedition/mods/266/files/725705?key=8WNi8WcqllkfAxbdkL3L0w&expires=1773066593&user_id=8510191";
        let nxm = NxmUrl::parse(url).unwrap();
        assert_eq!(nxm.game_domain, "skyrimspecialedition");
        assert_eq!(nxm.mod_id, 266);
        assert_eq!(nxm.file_id, 725705);
        assert_eq!(nxm.key.as_deref(), Some("8WNi8WcqllkfAxbdkL3L0w"));
        assert_eq!(nxm.expires.as_deref(), Some("1773066593"));
    }

    #[test]
    fn parse_nxm_url_without_query() {
        let url = "nxm://fallout4/mods/100/files/200";
        let nxm = NxmUrl::parse(url).unwrap();
        assert_eq!(nxm.game_domain, "fallout4");
        assert_eq!(nxm.mod_id, 100);
        assert_eq!(nxm.file_id, 200);
        assert!(nxm.key.is_none());
        assert!(nxm.expires.is_none());
    }
}
