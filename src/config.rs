use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::BufReader;
use std::path::Path;

use crate::error;

/// The structure used to store shell part of the configuration
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TemperatureConfig {
    pub device: Option<String>,
    pub pattern: Option<String>,
}

/// The structure used to store JSON part of the configuration
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct JsonConfig {
    pub enabled: Option<bool>,
}

/// The structure used to store shell part of the configuration
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ShellConfig {
    pub enabled: Option<bool>,
}

/// The structure used to store configuration of a single module
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ModuleConfig {
    pub enabled: Option<bool>,
    pub timeout_s: Option<u64>,
    pub temperature: Option<TemperatureConfig>,
    pub json: Option<JsonConfig>,
    pub shell: Option<ShellConfig>,
}

impl ModuleConfig {
    pub fn new() -> Self {
        Self {
            enabled: None,
            timeout_s: None,
            temperature: None,
            json: None,
            shell: None,
        }
    }
}

/// The structure used to store configuration of modules
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Config {
    pub modules: HashMap<String, ModuleConfig>,
}

/// Function used to load the configuration from a file
pub fn load<P: AsRef<Path>>(path: P) -> Result<Config, error::CerebroError> {
    // Open the file in read-only mode
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return error!("Cannot open config"),
    };

    let reader = BufReader::new(file);

    // Read the JSON contents of the file
    match serde_json::from_reader(reader) {
        Ok(c) => return Ok(c),
        Err(_) => return error!("Cannot parse Json config"),
    };
}
