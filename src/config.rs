use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    pub mappings: HashMap<PathBuf, Vec<PathBuf>>,
}

impl Config {
    pub fn load() -> Result<Self> {
        let config_path = Self::config_file_path()?;
        
        if !config_path.exists() {
            return Ok(Self {
                mappings: HashMap::new(),
            });
        }
        
        let content = fs::read_to_string(&config_path)?;
        let config: Self = serde_json::from_str(&content)?;
        Ok(config)
    }
    
    pub fn save(&self) -> Result<()> {
        let config_path = Self::config_file_path()?;
        
        if let Some(parent) = config_path.parent() {
            fs::create_dir_all(parent)?;
        }
        
        let content = serde_json::to_string_pretty(self)?;
        fs::write(&config_path, content)?;
        Ok(())
    }
    
    pub fn add_mapping(&mut self, source: PathBuf, destination: PathBuf) -> Result<()> {
        let source = source.canonicalize()?;
        let dest_file = if destination.is_dir() {
            destination.join(source.file_name().context("Invalid source filename")?)
        } else {
            destination
        };
        let dest_file = dest_file.canonicalize().unwrap_or(dest_file);
        
        self.mappings
            .entry(source)
            .or_default()
            .push(dest_file);
        
        self.save()?;
        Ok(())
    }
    
    pub fn remove_mapping(&mut self, file: &Path) -> Result<bool> {
        let file = file.canonicalize()?;
        let mut removed = false;
        
        self.mappings.retain(|_source, destinations| {
            destinations.retain(|dest| {
                if dest == &file {
                    removed = true;
                    false
                } else {
                    true
                }
            });
            !destinations.is_empty()
        });
        
        for (_, destinations) in self.mappings.iter_mut() {
            let initial_len = destinations.len();
            destinations.retain(|dest| dest != &file);
            if destinations.len() < initial_len {
                removed = true;
            }
        }
        
        if removed {
            self.save()?;
        }
        
        Ok(removed)
    }
    
    pub fn list_mappings(&self) -> Vec<(PathBuf, Vec<PathBuf>)> {
        self.mappings
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }
    
    fn config_file_path() -> Result<PathBuf> {
        let config_dir = dirs::config_dir()
            .context("Could not determine config directory")?;
        Ok(config_dir.join("mdman").join("config.json"))
    }
}