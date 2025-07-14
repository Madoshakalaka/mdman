use anyhow::Result;
use notify::{Config as NotifyConfig, Event, RecommendedWatcher, RecursiveMode, Watcher};
use notify_rust::Notification;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crate::config::Config;

pub struct FileWatcher {
    config: Config,
    reverse_mappings: HashMap<PathBuf, PathBuf>,
}

impl FileWatcher {
    pub fn new() -> Result<Self> {
        let config = Config::load()?;
        let mut reverse_mappings = HashMap::new();
        
        for (source, destinations) in config.mappings.iter() {
            for dest in destinations {
                reverse_mappings.insert(dest.clone(), source.clone());
            }
        }
        
        Ok(Self { config, reverse_mappings })
    }
    
    pub fn run(&mut self) -> Result<()> {
        let (tx, rx) = mpsc::channel();
        
        let mut watcher = RecommendedWatcher::new(tx, NotifyConfig::default())?;
        
        let mut watched_count = 0;
        
        for (source_file, destinations) in &self.config.mappings {
            if source_file.exists() {
                watcher.watch(source_file, RecursiveMode::NonRecursive)?;
                watched_count += 1;
            }
            
            for dest_file in destinations {
                if dest_file.exists() {
                    watcher.watch(dest_file, RecursiveMode::NonRecursive)?;
                    watched_count += 1;
                }
            }
        }
        
        println!("Watching {watched_count} files for changes...");
        
        loop {
            match rx.recv() {
                Ok(event) => {
                    if let Err(e) = self.handle_event(event) {
                        eprintln!("Error handling event: {e}");
                    }
                }
                Err(e) => {
                    eprintln!("Watch error: {e}");
                    thread::sleep(Duration::from_secs(1));
                }
            }
        }
    }
    
    fn handle_event(&mut self, event: Result<Event, notify::Error>) -> Result<()> {
        let event = event?;
        
        if !matches!(
            event.kind,
            notify::EventKind::Modify(_) | notify::EventKind::Create(_)
        ) {
            return Ok(());
        }
        
        self.config = Config::load()?;
        self.update_reverse_mappings();
        
        for path in event.paths {
            let canonical_path = path.canonicalize().unwrap_or(path.clone());
            
            if self.config.mappings.contains_key(&canonical_path) {
                self.sync_file(&canonical_path)?;
            } else if let Some(source) = self.reverse_mappings.get(&canonical_path) {
                self.warn_desync(&canonical_path, source)?;
            }
        }
        
        Ok(())
    }
    
    fn update_reverse_mappings(&mut self) {
        self.reverse_mappings.clear();
        for (source, destinations) in self.config.mappings.iter() {
            for dest in destinations {
                self.reverse_mappings.insert(dest.clone(), source.clone());
            }
        }
    }
    
    fn sync_file(&mut self, source_path: &Path) -> Result<()> {
        let canonical_source = source_path.canonicalize()?;
        
        if let Some(destinations) = self.config.mappings.get(&canonical_source) {
            let content = fs::read_to_string(&canonical_source)?;
            let mut synced_files = Vec::new();
            
            for dest in destinations {
                match fs::write(dest, &content) {
                    Ok(_) => {
                        synced_files.push(dest.clone());
                    }
                    Err(e) => {
                        eprintln!("Failed to sync to {}: {}", dest.display(), e);
                    }
                }
            }
            
            if !synced_files.is_empty() {
                self.send_notification(&canonical_source, &synced_files)?;
            }
        }
        
        Ok(())
    }
    
    fn send_notification(&self, source: &Path, destinations: &[PathBuf]) -> Result<()> {
        let source_name = source.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");
        
        let dest_count = destinations.len();
        let message = if dest_count == 1 {
            format!("Synced {source_name} to 1 location")
        } else {
            format!("Synced {source_name} to {dest_count} locations")
        };
        
        Notification::new()
            .summary("mdman: File synced")
            .body(&message)
            .icon("document-save")
            .timeout(3000)
            .show()?;
        
        println!("{message}");
        
        Ok(())
    }
    
    fn warn_desync(&self, dest_path: &Path, source_path: &Path) -> Result<()> {
        let dest_name = dest_path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");
        
        let message = format!(
            "Warning: {} was modified directly!\nSource: {}\nUse 'mdman sync' to re-sync from source or 'mdman diff' to see differences",
            dest_name,
            source_path.display()
        );
        
        Notification::new()
            .summary("mdman: Desync detected!")
            .body(&message)
            .icon("dialog-warning")
            .urgency(notify_rust::Urgency::Critical)
            .timeout(0)
            .show()?;
        
        eprintln!("\n{message}\n");
        
        Ok(())
    }
}