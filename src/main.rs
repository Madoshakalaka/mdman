use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::fs;
use std::path::PathBuf;

mod config;
mod watcher;

use config::Config;
use watcher::FileWatcher;

#[derive(Parser)]
#[command(name = "mdman")]
#[command(about = "Markdown file manager for keeping files in sync", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    #[command(about = "Install mdman as a systemd service")]
    Install,
    
    #[command(about = "Copy a source file to destination and track it for synchronization")]
    Copy {
        #[arg(help = "Source markdown file path")]
        source: PathBuf,
        #[arg(help = "Destination directory")]
        destination: PathBuf,
    },
    
    #[command(about = "List all tracked files")]
    List,
    
    #[command(about = "Stop tracking a file")]
    Untrack {
        #[arg(help = "File path to stop tracking")]
        file: PathBuf,
    },
    
    #[command(about = "Run the file watcher service")]
    Watch,
    
    #[command(about = "Synchronize all tracked files from source to destination")]
    Sync,
    
    #[command(about = "Show differences between source and destination files")]
    Diff {
        #[arg(help = "Optional specific file to check (checks all if not specified)")]
        file: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    
    match cli.command {
        Commands::Install => install_service(),
        Commands::Copy { source, destination } => copy_and_track(source, destination),
        Commands::List => list_tracked_files(),
        Commands::Untrack { file } => untrack_file(file),
        Commands::Watch => run_watcher(),
        Commands::Sync => sync_all_files(),
        Commands::Diff { file } => show_diff(file),
    }
}

fn copy_and_track(source: PathBuf, destination: PathBuf) -> Result<()> {
    if !source.exists() {
        anyhow::bail!("Source file {} does not exist", source.display());
    }
    
    if !source.is_file() {
        anyhow::bail!("Source {} is not a file", source.display());
    }
    
    let dest_path = if destination.is_dir() {
        let filename = source.file_name()
            .context("Invalid source filename")?;
        destination.join(filename)
    } else {
        destination.clone()
    };
    
    if let Some(parent) = dest_path.parent() {
        fs::create_dir_all(parent)
            .context("Failed to create destination directory")?;
    }
    
    fs::copy(&source, &dest_path)
        .with_context(|| format!("Failed to copy {} to {}", source.display(), dest_path.display()))?;
    
    let mut config = Config::load()?;
    config.add_mapping(source.clone(), destination)?;
    
    println!("Copied {} to {}", source.display(), dest_path.display());
    println!("File is now being tracked for synchronization");
    
    Ok(())
}

fn list_tracked_files() -> Result<()> {
    let config = Config::load()?;
    let mappings = config.list_mappings();
    
    if mappings.is_empty() {
        println!("No files are currently being tracked");
        return Ok(());
    }
    
    println!("Tracked files:");
    println!();
    
    for (source, destinations) in mappings {
        println!("Source: {}", source.display());
        for dest in destinations {
            println!("  → {}", dest.display());
        }
        println!();
    }
    
    Ok(())
}

fn untrack_file(file: PathBuf) -> Result<()> {
    let mut config = Config::load()?;
    
    if config.remove_mapping(&file)? {
        println!("Stopped tracking {}", file.display());
    } else {
        println!("File {} was not being tracked", file.display());
    }
    
    Ok(())
}

fn install_service() -> Result<()> {
    let service_content = r#"[Unit]
Description=mdman - Markdown file synchronization manager
After=graphical-session.target

[Service]
Type=simple
ExecStart=/usr/local/bin/mdman watch
Restart=on-failure
RestartSec=10
Environment="DISPLAY=:0"

[Install]
WantedBy=default.target"#;
    
    let service_path = dirs::config_dir()
        .context("Could not determine config directory")?
        .join("systemd/user/mdman.service");
    
    let service_exists = service_path.exists();
    
    if let Some(parent) = service_path.parent() {
        fs::create_dir_all(parent)
            .context("Failed to create systemd user directory")?;
    }
    
    fs::write(&service_path, service_content)
        .context("Failed to write systemd service file")?;
    
    let exe_path = std::env::current_exe()
        .context("Failed to get current executable path")?;
    
    let install_path = PathBuf::from("/usr/local/bin/mdman");
    
    if exe_path != install_path {
        println!("Installing mdman to /usr/local/bin/mdman (requires sudo)...");
        
        let status = std::process::Command::new("sudo")
            .args(["cp", exe_path.to_str().unwrap(), "/usr/local/bin/mdman"])
            .status()
            .context("Failed to copy executable")?;
        
        if !status.success() {
            anyhow::bail!("Failed to install mdman to /usr/local/bin/");
        }
        
        std::process::Command::new("sudo")
            .args(["chmod", "+x", "/usr/local/bin/mdman"])
            .status()
            .context("Failed to make executable")?;
    }
    
    if service_exists {
        println!("Updating existing mdman systemd service...");
        
        std::process::Command::new("systemctl")
            .args(["--user", "stop", "mdman.service"])
            .status()
            .context("Failed to stop existing service")?;
    } else {
        println!("Installing mdman systemd service...");
    }
    
    std::process::Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status()
        .context("Failed to reload systemd")?;
    
    std::process::Command::new("systemctl")
        .args(["--user", "enable", "mdman.service"])
        .status()
        .context("Failed to enable service")?;
    
    std::process::Command::new("systemctl")
        .args(["--user", "start", "mdman.service"])
        .status()
        .context("Failed to start service")?;
    
    if service_exists {
        println!("mdman service updated and restarted successfully!");
    } else {
        println!("mdman service installed and started successfully!");
    }
    println!("Use 'systemctl --user status mdman' to check service status");
    
    Ok(())
}

fn run_watcher() -> Result<()> {
    let mut watcher = FileWatcher::new()?;
    watcher.run()?;
    Ok(())
}

fn sync_all_files() -> Result<()> {
    let config = Config::load()?;
    let mappings = config.list_mappings();
    
    if mappings.is_empty() {
        println!("No files are currently being tracked");
        return Ok(());
    }
    
    let mut synced_count = 0;
    let mut error_count = 0;
    
    for (source, destinations) in mappings {
        if !source.exists() {
            eprintln!("Warning: Source file {} does not exist", source.display());
            error_count += 1;
            continue;
        }
        
        let content = match fs::read(&source) {
            Ok(content) => content,
            Err(e) => {
                eprintln!("Error reading {}: {}", source.display(), e);
                error_count += 1;
                continue;
            }
        };
        
        for dest in destinations {
            match fs::write(&dest, &content) {
                Ok(_) => {
                    println!("Synced {} → {}", source.display(), dest.display());
                    synced_count += 1;
                }
                Err(e) => {
                    eprintln!("Error syncing to {}: {}", dest.display(), e);
                    error_count += 1;
                }
            }
        }
    }
    
    println!();
    println!("Synchronization complete: {synced_count} files synced");
    if error_count > 0 {
        println!("{error_count} errors occurred");
    }
    
    Ok(())
}

fn show_diff(file: Option<PathBuf>) -> Result<()> {
    let config = Config::load()?;
    let mappings = config.list_mappings();
    
    if mappings.is_empty() {
        println!("No files are currently being tracked");
        return Ok(());
    }
    
    let mut diff_found = false;
    
    for (source, destinations) in mappings {
        if let Some(ref specific_file) = file {
            let canonical_specific = specific_file.canonicalize().unwrap_or(specific_file.clone());
            let matches_source = source == canonical_specific;
            let matches_dest = destinations.iter().any(|d| d == &canonical_specific);
            
            if !matches_source && !matches_dest {
                continue;
            }
        }
        
        if !source.exists() {
            println!("Source file {} does not exist", source.display());
            diff_found = true;
            continue;
        }
        
        let source_content = match fs::read(&source) {
            Ok(content) => content,
            Err(e) => {
                eprintln!("Error reading {}: {}", source.display(), e);
                continue;
            }
        };
        
        for dest in destinations {
            if !dest.exists() {
                println!("Destination {} does not exist (source: {})", dest.display(), source.display());
                diff_found = true;
                continue;
            }
            
            let dest_content = match fs::read(&dest) {
                Ok(content) => content,
                Err(e) => {
                    eprintln!("Error reading {}: {}", dest.display(), e);
                    continue;
                }
            };
            
            if source_content != dest_content {
                println!("Files differ:");
                println!("  Source: {}", source.display());
                println!("  Dest:   {}", dest.display());
                
                let source_size = source_content.len();
                let dest_size = dest_content.len();
                println!("  Size difference: {source_size} vs {dest_size} bytes");
                
                diff_found = true;
            }
        }
    }
    
    if !diff_found {
        if file.is_some() {
            println!("No differences found for the specified file");
        } else {
            println!("All tracked files are in sync");
        }
    }
    
    Ok(())
}
