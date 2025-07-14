use anyhow::Result;
use notify::{Config as NotifyConfig, Event, RecommendedWatcher, RecursiveMode, Watcher};
use notify_rust::Notification;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use tracing::{error, info, instrument, warn};

use crate::config::Config;

pub struct FileWatcher {
    config: Config,
    reverse_mappings: HashMap<PathBuf, PathBuf>,
    last_known_content: HashMap<PathBuf, Vec<u8>>,
    recently_synced: HashMap<PathBuf, Instant>,
}

impl FileWatcher {
    #[instrument]
    pub fn new() -> Result<Self> {
        let config = Config::load()?;
        let mut reverse_mappings = HashMap::new();
        let mut last_known_content = HashMap::new();
        
        for (source, destinations) in config.mappings.iter() {
            for dest in destinations {
                reverse_mappings.insert(dest.clone(), source.clone());
            }
            
            // Initialize with current content
            if source.exists() {
                if let Ok(content) = fs::read(source) {
                    last_known_content.insert(source.clone(), content);
                }
            }
        }
        
        Ok(Self { 
            config, 
            reverse_mappings, 
            last_known_content,
            recently_synced: HashMap::new(),
        })
    }
    
    #[instrument(skip(self))]
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
        
        info!("Watching {watched_count} files for changes...");
        
        loop {
            match rx.recv() {
                Ok(event) => {
                    if let Err(e) = self.handle_event(event) {
                        error!("Error handling event: {e}");
                    }
                }
                Err(e) => {
                    error!("Watch error: {e}");
                    thread::sleep(Duration::from_secs(1));
                }
            }
        }
    }
    
    #[instrument(skip(self, event))]
    fn handle_event(&mut self, event: Result<Event, notify::Error>) -> Result<()> {
        let event = event?;
        
        if !matches!(
            event.kind,
            notify::EventKind::Modify(_) | notify::EventKind::Create(_) | notify::EventKind::Remove(_)
        ) {
            return Ok(());
        }
        
        self.config = Config::load()?;
        self.update_reverse_mappings();
        
        // Clean up old entries from recently_synced (older than 5 seconds)
        let now = Instant::now();
        self.recently_synced.retain(|_, sync_time| {
            now.duration_since(*sync_time) < Duration::from_secs(5)
        });
        
        for path in event.paths {
            // Handle file removal
            if matches!(event.kind, notify::EventKind::Remove(_)) {
                // Check if it's a source file that was removed
                if let Some(destinations) = self.config.mappings.get(&path).cloned() {
                    self.warn_source_deleted(&path, &destinations)?;
                    
                    // Remove the deleted source from config
                    self.config.mappings.remove(&path);
                    
                    // Save the updated config to persist the removal
                    if let Err(e) = self.config.save() {
                        error!("Failed to save config after removing deleted source: {}", e);
                    }
                    
                    // Update reverse mappings to stop watching the destination files
                    for dest in destinations {
                        self.reverse_mappings.remove(&dest);
                    }
                }
                continue;
            }
            
            let canonical_path = path.canonicalize().unwrap_or(path.clone());
            
            if self.config.mappings.contains_key(&canonical_path) {
                self.sync_file(&canonical_path)?;
            } else if let Some(source) = self.reverse_mappings.get(&canonical_path) {
                // Check if this file was recently synced (within 2 seconds)
                if let Some(sync_time) = self.recently_synced.get(&canonical_path) {
                    if sync_time.elapsed() < Duration::from_secs(2) {
                        // Skip warning - this is likely our own modification
                        continue;
                    }
                }
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
    
    #[instrument(skip(self), fields(source = %source_path.display()))]
    fn sync_file(&mut self, source_path: &Path) -> Result<()> {
        let canonical_source = source_path.canonicalize()?;
        
        if let Some(destinations) = self.config.mappings.get(&canonical_source) {
            // Read old content before the change for comparison
            let old_source_content = self.last_known_content.get(&canonical_source)
                .cloned()
                .unwrap_or_else(Vec::new);
            
            let source_content = fs::read(&canonical_source)?;
            
            // Store new content for next time
            self.last_known_content.insert(canonical_source.clone(), source_content.clone());
            
            let mut synced_files = Vec::new();
            let mut desynced_files = Vec::new();
            
            for dest in destinations {
                if dest.exists() {
                    let dest_content = fs::read(dest).unwrap_or_default();
                    
                    // Check if destination was in sync with the OLD source content
                    let was_in_sync = dest_content == old_source_content || old_source_content.is_empty();
                    
                    if was_in_sync {
                        // File was in sync, so update it
                        match fs::write(dest, &source_content) {
                            Ok(_) => {
                                synced_files.push(dest.clone());
                                // Mark this file as recently synced
                                self.recently_synced.insert(dest.clone(), Instant::now());
                            }
                            Err(e) => {
                                error!("Failed to sync to {}: {}", dest.display(), e);
                            }
                        }
                    } else {
                        // File was not in sync, leave it alone
                        desynced_files.push(dest.clone());
                    }
                } else {
                    // Create new file
                    if let Some(parent) = dest.parent() {
                        let _ = fs::create_dir_all(parent);
                    }
                    match fs::write(dest, &source_content) {
                        Ok(_) => {
                            synced_files.push(dest.clone());
                            // Mark this file as recently synced
                            self.recently_synced.insert(dest.clone(), Instant::now());
                        }
                        Err(e) => {
                            error!("Failed to create {}: {}", dest.display(), e);
                        }
                    }
                }
            }
            
            if !synced_files.is_empty() || !desynced_files.is_empty() {
                self.send_sync_notification(&canonical_source, &synced_files, &desynced_files)?;
            }
        }
        
        Ok(())
    }
    
    
    fn send_sync_notification(&self, source: &Path, synced_files: &[PathBuf], desynced_files: &[PathBuf]) -> Result<()> {
        let source_name = source.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");
        
        let synced_count = synced_files.len();
        let desynced_count = desynced_files.len();
        
        let mut message = if synced_count == 1 {
            format!("{} file has been synced", synced_count)
        } else if synced_count > 1 {
            format!("{} files have been synced", synced_count)
        } else {
            String::new()
        };
        
        if desynced_count > 0 {
            if !message.is_empty() {
                message.push_str(", ");
            }
            if desynced_count == 1 {
                message.push_str(&format!("{} desynced file left out", desynced_count));
            } else {
                message.push_str(&format!("{} desynced files left out", desynced_count));
            }
        }
        
        if !message.is_empty() {
            Notification::new()
                .summary(&format!("mdman: {}", source_name))
                .body(&message)
                .icon(if desynced_count > 0 { "dialog-warning" } else { "document-save" })
                .timeout(3000)
                .show()?;
            
            info!("{}: {}", source_name, message);
            
            if desynced_count > 0 {
                warn!("Desynced files:");
                for file in desynced_files {
                    warn!("  - {}", file.display());
                }
                warn!("Use 'mdman sync' to force sync or 'mdman diff' to see differences");
            }
        }
        
        Ok(())
    }
    
    #[instrument(skip(self), fields(dest = %dest_path.display(), source = %source_path.display()))]
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
        
        warn!("{message}");
        
        Ok(())
    }
    
    #[instrument(skip(self, destinations), fields(source = %source_path.display(), dest_count = destinations.len()))]
    fn warn_source_deleted(&self, source_path: &Path, destinations: &[PathBuf]) -> Result<()> {
        let source_name = source_path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");
        
        let dest_count = destinations.len();
        let message = if dest_count == 1 {
            format!(
                "Source file {} was deleted!\nDestination file remains at:\n{}",
                source_name,
                destinations[0].display()
            )
        } else {
            let dest_list: Vec<String> = destinations.iter()
                .map(|d| format!("  - {}", d.display()))
                .collect();
            format!(
                "Source file {} was deleted!\n{} destination files remain at:\n{}",
                source_name,
                dest_count,
                dest_list.join("\n")
            )
        };
        
        Notification::new()
            .summary("mdman: Source file deleted!")
            .body(&message)
            .icon("dialog-warning")
            .urgency(notify_rust::Urgency::Critical)
            .timeout(0)
            .show()?;
        
        warn!("{}", message);
        warn!("Note: Destination files were not deleted and are no longer being watched.");
        warn!("The tracking for {} has been automatically removed.", source_path.display());
        
        Ok(())
    }
}