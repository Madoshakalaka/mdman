use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use tracing::instrument;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use mdman_service::{Config, FileWatcher, DiffReport};

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
    
    #[command(about = "Remove source file and all its destination files")]
    Remove {
        #[arg(help = "Source file to remove along with all destinations")]
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
    // Initialize tracing with journald
    tracing_subscriber::registry()
        .with(tracing_journald::layer().unwrap())
        .init();
    
    let cli = Cli::parse();
    
    match cli.command {
        Commands::Install => install_service(),
        Commands::Copy { source, destination } => copy_and_track(source, destination),
        Commands::List => list_tracked_files(),
        Commands::Untrack { file } => untrack_file(file),
        Commands::Remove { file } => remove_file(file),
        Commands::Watch => run_watcher(),
        Commands::Sync => sync_all_files(),
        Commands::Diff { file } => show_diff(file),
    }
}

#[instrument(skip_all, fields(source = %source.display(), destination = %destination.display()))]
fn copy_and_track(source: PathBuf, destination: PathBuf) -> Result<()> {
    if !source.exists() {
        anyhow::bail!("Source file {} does not exist", source.display());
    }
    
    if !source.is_file() {
        anyhow::bail!("Source {} is not a file", source.display());
    }
    
    let config = Config::load()?;
    let canonical_source = source.canonicalize()?;
    
    // Check if source is already being tracked (either as source or destination)
    if config.mappings.contains_key(&canonical_source) {
        anyhow::bail!("{} is already being tracked as a source file", source.display());
    }
    
    for (_, destinations) in config.mappings.iter() {
        if destinations.iter().any(|d| d == &canonical_source) {
            anyhow::bail!("{} is already being tracked as a destination file", source.display());
        }
    }
    
    let dest_path = if destination.is_dir() {
        let filename = source.file_name()
            .context("Invalid source filename")?;
        destination.join(filename)
    } else {
        destination.clone()
    };
    
    // Check if destination is already being tracked
    let canonical_dest = dest_path.canonicalize().unwrap_or(dest_path.clone());
    
    if config.mappings.contains_key(&canonical_dest) {
        anyhow::bail!("{} is already being tracked as a source file", dest_path.display());
    }
    
    for (_, destinations) in config.mappings.iter() {
        if destinations.iter().any(|d| d == &canonical_dest) {
            anyhow::bail!("{} is already being tracked as a destination file", dest_path.display());
        }
    }
    
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

#[instrument]
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
    let config = Config::load()?;
    
    // Check if it's a source file
    if let Some((source_path, destinations)) = config.find_by_path(&file) {
        let dest_count = destinations.len();
        println!("{} is a source file for {} destination(s):", file.display(), dest_count);
        for dest in destinations {
            println!("  → {}", dest.display());
        }
        
        print!("\nRemove tracking for all {} destination files? [y/N] ", dest_count);
        io::stdout().flush()?;
        
        let mut response = String::new();
        io::stdin().read_line(&mut response)?;
        
        if response.trim().to_lowercase() == "y" {
            let mut config = Config::load()?;
            config.mappings.remove(&source_path);
            config.save()?;
            println!("Stopped tracking {} and all its destinations", file.display());
        } else {
            println!("Cancelled");
        }
        return Ok(());
    }
    
    // Check if it's a destination file
    let canonical_file = file.canonicalize().unwrap_or_else(|_| file.clone());
    for (source, destinations) in config.mappings.iter() {
        let matches = destinations.iter().any(|d| {
            d == &canonical_file || 
            d.canonicalize().unwrap_or_else(|_| d.clone()) == canonical_file ||
            (file.exists() && d.canonicalize().ok() == file.canonicalize().ok())
        });
        
        if matches {
            println!("{} is a destination file tracked from source:", file.display());
            println!("  ← {}", source.display());
            
            print!("\nStop tracking this destination? [y/N] ");
            io::stdout().flush()?;
            
            let mut response = String::new();
            io::stdin().read_line(&mut response)?;
            
            if response.trim().to_lowercase() == "y" {
                let mut config = Config::load()?;
                config.remove_mapping(&file)?;
                println!("Stopped tracking {}", file.display());
            } else {
                println!("Cancelled");
            }
            return Ok(());
        }
    }
    
    println!("File {} is not being tracked", file.display());
    Ok(())
}

fn remove_file(file: PathBuf) -> Result<()> {
    let config = Config::load()?;
    
    // Check if it's a source file
    if let Some((source_path, destinations)) = config.find_by_path(&file) {
        let dest_count = destinations.len();
        
        println!("{} is a source file with {} destination(s):", file.display(), dest_count);
        for dest in &destinations {
            println!("  → {}", dest.display());
        }
        
        println!("\nThis will DELETE:");
        println!("  - {} (source)", source_path.display());
        for dest in &destinations {
            println!("  - {} (destination)", dest.display());
        }
        
        print!("\nPERMANENTLY DELETE all {} files? [y/N] ", dest_count + 1);
        io::stdout().flush()?;
        
        let mut response = String::new();
        io::stdin().read_line(&mut response)?;
        
        if response.trim().to_lowercase() == "y" {
            // Delete source file
            if source_path.exists() {
                fs::remove_file(&source_path)
                    .with_context(|| format!("Failed to delete source file {}", source_path.display()))?;
                println!("Deleted source: {}", source_path.display());
            }
            
            // Delete destination files
            for dest in &destinations {
                if dest.exists() {
                    fs::remove_file(dest)
                        .with_context(|| format!("Failed to delete destination file {}", dest.display()))?;
                    println!("Deleted destination: {}", dest.display());
                }
            }
            
            // Remove from config
            let mut config = Config::load()?;
            config.mappings.remove(&source_path);
            config.save()?;
            
            println!("\nAll files deleted and tracking removed.");
        } else {
            println!("Cancelled - no files were deleted");
        }
    } else {
        println!("{} is not a tracked source file", file.display());
        println!("The remove command only works on source files.");
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
    if config.list_mappings().is_empty() {
        println!("No files are currently being tracked");
        return Ok(());
    }
    
    let stats = mdman_service::sync_all_files()?;
    
    println!();
    println!("Synchronization complete: {} files synced", stats.synced_count);
    if stats.error_count > 0 {
        println!("{} errors occurred", stats.error_count);
    }
    
    Ok(())
}

fn show_diff(file: Option<PathBuf>) -> Result<()> {
    let config = Config::load()?;
    if config.list_mappings().is_empty() {
        println!("No files are currently being tracked");
        return Ok(());
    }
    
    let diffs = mdman_service::check_diff(file.as_deref())?;
    
    if diffs.is_empty() {
        if file.is_some() {
            println!("No differences found for the specified file");
        } else {
            println!("All tracked files are in sync");
        }
    } else {
        for diff in diffs {
            match diff {
                DiffReport::SourceMissing { source } => {
                    println!("Source file {} does not exist", source.display());
                }
                DiffReport::DestinationMissing { source, destination } => {
                    println!("Destination {} does not exist (source: {})", destination.display(), source.display());
                }
                DiffReport::ContentDiffers { source, destination, source_size, dest_size } => {
                    println!("Files differ:");
                    println!("  Source: {}", source.display());
                    println!("  Dest:   {}", destination.display());
                    println!("  Size difference: {} vs {} bytes", source_size, dest_size);
                }
            }
        }
    }
    
    Ok(())
}
