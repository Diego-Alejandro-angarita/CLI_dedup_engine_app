use clap::{Parser, Subcommand};
use colored::Colorize;
use serde::{Deserialize, Serialize};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use tokio::fs::{self, File, OpenOptions};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const BLOCK_SIZE: usize = 4096;
const FNV_PRIME: u64 = 1099511628211;
const FNV_OFFSET_BASIS: u64 = 14695981039346656037;

const FREE_MAX_FILE_SIZE: u64 = 300 * 1024 * 1024; // 300 MB
const FREE_MAX_REPO_SIZE: u64 = 1024 * 1024 * 1024; // 1 GB

#[derive(Parser)]
#[command(name = "dedup-engine", about = "Smart Deduplication Backup CLI", version = "1.0")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Backup a file
    Backup {
        /// File to backup
        file: String,
        /// Compress the backup (Pro feature)
        #[arg(long)]
        compress: bool,
        /// Encrypt the backup (Pro feature)
        #[arg(long)]
        encrypt: bool,
    },
    /// Restore a file from a recipe
    Restore {
        /// Recipe name
        recipe: String,
        /// Destination path
        destination: String,
    },
    /// Show repository statistics
    Stats,
}

#[derive(Serialize, Deserialize, Default, Clone)]
struct Metrics {
    logical_bytes: u64,
    stored_bytes: u64,
}

fn compute_chunk_hash(buffer: &[u8]) -> String {
    let mut hash = FNV_OFFSET_BASIS;
    for &byte in buffer {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    format!("{:016x}", hash)
}

fn get_repo_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".dedup")
}

fn get_metrics_path() -> PathBuf {
    get_repo_dir().join("metrics.json")
}

async fn init_repo_if_needed() -> std::io::Result<()> {
    let repo_dir = get_repo_dir();
    if !repo_dir.exists() {
        println!("{}", "No repository found. Initializing...".yellow());
        fs::create_dir_all(repo_dir.join("chunks")).await?;
        fs::create_dir_all(repo_dir.join("recipes")).await?;
        
        let initial_metrics = Metrics::default();
        let metrics_json = serde_json::to_string_pretty(&initial_metrics).unwrap();
        fs::write(get_metrics_path(), metrics_json).await?;
        
        println!("{} Repository created at ~/.dedup", "✔".green());
    }
    Ok(())
}

async fn load_metrics() -> Metrics {
    let path = get_metrics_path();
    if path.exists() {
        if let Ok(data) = fs::read_to_string(&path).await {
            if let Ok(metrics) = serde_json::from_str(&data) {
                return metrics;
            }
        }
    }
    Metrics::default()
}

async fn save_metrics(metrics: &Metrics) -> std::io::Result<()> {
    let path = get_metrics_path();
    let json = serde_json::to_string_pretty(metrics).unwrap();
    fs::write(path, json).await
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Backup { file, compress, encrypt } => {
            if compress || encrypt {
                println!("{} Feature requires Pro version.", "🔒".bright_red());
                return Ok(());
            }

            let path = Path::new(&file);
            if !path.exists() {
                println!("{} File not found: {}", "❌".red(), file);
                return Ok(());
            }

            let metadata = fs::metadata(&path).await?;
            if metadata.len() > FREE_MAX_FILE_SIZE {
                println!(
                    "{} File too large ({} MB). Free plan is limited to 300MB per file. Upgrade to Pro.",
                    "❌".red(),
                    metadata.len() / (1024 * 1024)
                );
                return Ok(());
            }

            init_repo_if_needed().await?;
            let mut metrics = load_metrics().await;

            if metrics.stored_bytes >= FREE_MAX_REPO_SIZE {
                println!("{} Storage limit reached (1GB). Upgrade to Pro for unlimited storage.", "❌".red());
                return Ok(());
            }

            let recipe_name = path.file_name().unwrap().to_string_lossy().to_string();
            let repo_dir = get_repo_dir();
            let recipe_path = repo_dir.join("recipes").join(format!("{}.recipe", recipe_name));

            let mut source_file = File::open(&path).await?;
            let mut recipe_file = File::create(&recipe_path).await?;

            let mut buffer = [0u8; BLOCK_SIZE];
            let mut new_chunks = 0;
            let mut dedup_chunks = 0;

            println!("🚀 Backing up '{}'...", file);

            loop {
                let bytes_read = source_file.read(&mut buffer).await?;
                if bytes_read == 0 {
                    break;
                }

                metrics.logical_bytes += bytes_read as u64;

                let mut chunk_data = buffer;
                if bytes_read < BLOCK_SIZE {
                    chunk_data[bytes_read..].fill(0);
                }

                let hash_str = compute_chunk_hash(&chunk_data);
                let chunk_path = repo_dir.join("chunks").join(&hash_str);

                let create_result = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&chunk_path)
                    .await;

                match create_result {
                    Ok(mut chunk_file) => {
                        chunk_file.write_all(&chunk_data).await?;
                        metrics.stored_bytes += BLOCK_SIZE as u64;
                        new_chunks += 1;
                        
                        if metrics.stored_bytes >= FREE_MAX_REPO_SIZE {
                            println!("{} Storage limit reached mid-backup! Upgrade to Pro.", "❌".red());
                            break;
                        }
                    }
                    Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                        dedup_chunks += 1;
                    }
                    Err(e) => return Err(e),
                }

                let recipe_entry = format!("{}\n", hash_str);
                recipe_file.write_all(recipe_entry.as_bytes()).await?;
            }

            save_metrics(&metrics).await?;
            
            let saved_mb = (dedup_chunks * BLOCK_SIZE) as f64 / (1024.0 * 1024.0);
            println!("{} Backup complete!", "✅".green());
            println!("   New chunks: {}", new_chunks);
            println!("   Deduplicated: {} (Saved {:.2} MB)", dedup_chunks, saved_mb);
        }
        Commands::Restore { recipe, destination } => {
            init_repo_if_needed().await?;
            let repo_dir = get_repo_dir();
            
            let recipe_filename = if recipe.ends_with(".recipe") {
                recipe.clone()
            } else {
                format!("{}.recipe", recipe)
            };
            
            let recipe_path = repo_dir.join("recipes").join(&recipe_filename);

            if !recipe_path.exists() {
                println!("{} Recipe '{}' not found.", "❌".red(), recipe);
                return Ok(());
            }

            let recipe_content = fs::read_to_string(&recipe_path).await?;
            let hashes: Vec<&str> = recipe_content.lines().filter(|l| !l.is_empty()).collect();
            let num_hashes = hashes.len();

            let mut dest_file = File::create(&destination).await?;
            println!("📥 Restoring to '{}'...", destination);

            for (i, hash) in hashes.iter().enumerate() {
                let chunk_path = repo_dir.join("chunks").join(hash);
                
                if !chunk_path.exists() {
                    println!("{} Corrupted backup: Missing chunk {}", "❌".red(), hash);
                    return Ok(());
                }

                let mut chunk_data = fs::read(&chunk_path).await?;

                if i == num_hashes - 1 {
                    while chunk_data.last() == Some(&0) {
                        chunk_data.pop();
                    }
                }

                dest_file.write_all(&chunk_data).await?;
            }

            println!("{} Restore complete!", "✅".green());
        }
        Commands::Stats => {
            init_repo_if_needed().await?;
            let metrics = load_metrics().await;
            
            let repo_mb = metrics.stored_bytes as f64 / (1024.0 * 1024.0);
            
            let saved_bytes = metrics.logical_bytes.saturating_sub(metrics.stored_bytes);
            let saved_gb = saved_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
            
            let pct = if metrics.logical_bytes > 0 {
                (saved_bytes as f64 / metrics.logical_bytes as f64 * 100.0) as u64
            } else {
                0
            };

            println!("\nRepository size: {:.0} MB / 1 GB", repo_mb);
            println!("Space saved: {:.1} GB ({}%)", saved_gb, pct);

            // Warning at 80% capacity (800MB)
            if repo_mb > 800.0 {
                println!("\n{} You are approaching the free plan limit", "⚠".yellow());
                println!("Upgrade to Pro for unlimited storage");
            } else {
                println!("\nUpgrade to Pro for unlimited storage and compression");
            }
        }
    }

    Ok(())
}
