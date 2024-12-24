use serde::Deserialize;
use std::fs;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub rpc_url: String,
    pub websocket_url: String,
    pub db_url: String,
}

impl Config {
    pub fn load() -> Result<Self, Box<dyn std::error::Error>> {
        let config_str = fs::read_to_string("config.yaml")?;
        let config: Config = serde_yaml::from_str(&config_str)?;
        Ok(config)
    }
}
