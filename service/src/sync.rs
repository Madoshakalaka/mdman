use anyhow::Result;
use std::fs;
use std::path::Path;
use tracing::{error, info, instrument, warn};

use crate::config::Config;

pub struct SyncStats {
    pub synced_count: usize,
    pub error_count: usize,
}

#[instrument]
pub fn sync_all_files() -> Result<SyncStats> {
    let config = Config::load()?;
    let mappings = config.list_mappings();
    
    let mut synced_count = 0;
    let mut error_count = 0;
    
    for (source, destinations) in mappings {
        if !source.exists() {
            warn!("Source file {} does not exist", source.display());
            eprintln!("Warning: Source file {} does not exist", source.display());
            error_count += 1;
            continue;
        }
        
        let content = match fs::read(&source) {
            Ok(content) => content,
            Err(e) => {
                error!("Error reading {}: {}", source.display(), e);
                eprintln!("Error reading {}: {}", source.display(), e);
                error_count += 1;
                continue;
            }
        };
        
        for dest in destinations {
            match fs::write(&dest, &content) {
                Ok(_) => {
                    info!("Synced {} → {}", source.display(), dest.display());
                    println!("Synced {} → {}", source.display(), dest.display());
                    synced_count += 1;
                }
                Err(e) => {
                    error!("Error syncing to {}: {}", dest.display(), e);
                    eprintln!("Error syncing to {}: {}", dest.display(), e);
                    error_count += 1;
                }
            }
        }
    }
    
    Ok(SyncStats { synced_count, error_count })
}

#[instrument(skip_all, fields(file = ?file))]
pub fn check_diff(file: Option<&Path>) -> Result<Vec<DiffReport>> {
    let config = Config::load()?;
    let mappings = config.list_mappings();
    
    let mut diffs = Vec::new();
    
    for (source, destinations) in mappings {
        if let Some(specific_file) = file {
            let canonical_specific = specific_file.canonicalize().unwrap_or_else(|_| specific_file.to_path_buf());
            let matches_source = source == canonical_specific;
            let matches_dest = destinations.iter().any(|d| d == &canonical_specific);
            
            if !matches_source && !matches_dest {
                continue;
            }
        }
        
        if !source.exists() {
            diffs.push(DiffReport::SourceMissing { source: source.clone() });
            continue;
        }
        
        let source_content = match fs::read(&source) {
            Ok(content) => content,
            Err(e) => {
                error!("Error reading {}: {}", source.display(), e);
                continue;
            }
        };
        
        for dest in destinations {
            if !dest.exists() {
                diffs.push(DiffReport::DestinationMissing {
                    source: source.clone(),
                    destination: dest.clone(),
                });
                continue;
            }
            
            let dest_content = match fs::read(&dest) {
                Ok(content) => content,
                Err(e) => {
                    error!("Error reading {}: {}", dest.display(), e);
                    continue;
                }
            };
            
            if source_content != dest_content {
                diffs.push(DiffReport::ContentDiffers {
                    source: source.clone(),
                    destination: dest.clone(),
                    source_size: source_content.len(),
                    dest_size: dest_content.len(),
                });
            }
        }
    }
    
    Ok(diffs)
}

#[derive(Debug)]
pub enum DiffReport {
    SourceMissing {
        source: std::path::PathBuf,
    },
    DestinationMissing {
        source: std::path::PathBuf,
        destination: std::path::PathBuf,
    },
    ContentDiffers {
        source: std::path::PathBuf,
        destination: std::path::PathBuf,
        source_size: usize,
        dest_size: usize,
    },
}