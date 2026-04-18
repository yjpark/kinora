use facet::Facet;

#[derive(Facet, Debug, Clone, PartialEq)]
pub struct Config {
    #[facet(rename = "repo-url")]
    pub repo_url: String,
}

#[derive(Debug)]
pub enum ConfigError {
    Parse(String),
    Serialize(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Parse(m) => write!(f, "failed to parse config.styx: {m}"),
            ConfigError::Serialize(m) => write!(f, "failed to serialize config: {m}"),
        }
    }
}

impl std::error::Error for ConfigError {}

impl Config {
    pub fn from_styx(input: &str) -> Result<Self, ConfigError> {
        facet_styx::from_str(input).map_err(|e| ConfigError::Parse(e.to_string()))
    }

    pub fn to_styx(&self) -> Result<String, ConfigError> {
        facet_styx::to_string(self).map_err(|e| ConfigError::Serialize(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_single_field() {
        let c = Config { repo_url: "https://github.com/edger-dev/kinora".into() };
        let s = c.to_styx().unwrap();
        let parsed = Config::from_styx(&s).unwrap();
        assert_eq!(parsed, c);
    }

    #[test]
    fn parses_inline_form() {
        let c = Config::from_styx("repo-url https://github.com/edger-dev/kinora").unwrap();
        assert_eq!(c.repo_url, "https://github.com/edger-dev/kinora");
    }

    #[test]
    fn serialized_contains_repo_url_key() {
        let c = Config { repo_url: "https://example.com/foo.git".into() };
        let s = c.to_styx().unwrap();
        assert!(s.contains("repo-url"), "got: {s}");
        assert!(s.contains("https://example.com/foo.git"), "got: {s}");
    }
}
