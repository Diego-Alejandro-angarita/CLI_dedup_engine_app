use chrono::Utc;
use clap::{Parser, Subcommand};
use colored::Colorize;
use comfy_table::{Attribute, Cell, Color, Table};
use indicatif::{ProgressBar, ProgressStyle};
use notify::{EventKind, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::mpsc::channel;
use std::time::Duration;
use tokio::fs::{self, File};
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
    /// Authenticate Pro license
    Auth {
        /// Pro License Key
        key: String,
    },
    /// Watch a file and auto-backup changes (Pro feature)
    Watch {
        /// File to watch
        file: String,
        /// Compress the backups
        #[arg(long)]
        compress: bool,
    },
}

#[derive(Serialize, Deserialize, Default, Clone)]
struct BackupRecord {
    timestamp: String,
    recipe: String,
    size_mb: f64,
}

#[derive(Serialize, Deserialize, Default, Clone)]
struct Metrics {
    logical_bytes: u64,
    stored_bytes: u64,
    is_pro: bool,
    history: Vec<BackupRecord>,
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

async fn perform_backup(file: &str, recipe_name: &str, compress: bool, metrics: &mut Metrics) -> std::io::Result<()> {
    let path = Path::new(file);
    if !path.exists() {
        println!("{} File not found: {}", "❌".red(), file);
        return Ok(());
    }

    let metadata = fs::metadata(&path).await?;
    if !metrics.is_pro && metadata.len() > FREE_MAX_FILE_SIZE {
        println!(
            "{} File too large ({} MB). Free plan is limited to 300MB per file. Upgrade to Pro.",
            "❌".red(),
            metadata.len() / (1024 * 1024)
        );
        return Ok(());
    }

    if !metrics.is_pro && metrics.stored_bytes >= FREE_MAX_REPO_SIZE {
        println!("{} Storage limit reached (1GB). Upgrade to Pro for unlimited storage.", "❌".red());
        return Ok(());
    }

    let repo_dir = get_repo_dir();
    let recipe_path = repo_dir.join("recipes").join(format!("{}.recipe", recipe_name));

    let mut source_file = File::open(&path).await?;
    let mut recipe_file = File::create(&recipe_path).await?;

    let mut buffer = [0u8; BLOCK_SIZE];
    let mut new_chunks = 0;
    let mut dedup_chunks = 0;

    println!("\n{} Backing up '{}' as '{}'...", "🚀".cyan().bold(), file.bold(), recipe_name.bold());

    let pb = ProgressBar::new(metadata.len());
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {bytes}/{total_bytes} ({eta})",
        )
        .unwrap()
        .progress_chars("#>-"),
    );

    loop {
        let bytes_read = source_file.read(&mut buffer).await?;
        if bytes_read == 0 {
            break;
        }

        pb.inc(bytes_read as u64);
        metrics.logical_bytes += bytes_read as u64;

        let mut chunk_data = buffer;
        if bytes_read < BLOCK_SIZE {
            chunk_data[bytes_read..].fill(0);
        }

        let hash_str = compute_chunk_hash(&chunk_data);
        
        let chunk_file_name = if compress { format!("{}.zst", hash_str) } else { hash_str.clone() };
        let chunk_path = repo_dir.join("chunks").join(&chunk_file_name);

        let exists_uncompressed = repo_dir.join("chunks").join(&hash_str).exists();
        let exists_compressed = repo_dir.join("chunks").join(format!("{}.zst", hash_str)).exists();

        if exists_uncompressed || exists_compressed {
            dedup_chunks += 1;
        } else {
            let final_data = if compress {
                zstd::stream::encode_all(Cursor::new(&chunk_data), 3)?
            } else {
                chunk_data.to_vec()
            };

            fs::write(&chunk_path, &final_data).await?;
            metrics.stored_bytes += final_data.len() as u64;
            new_chunks += 1;
            
            if !metrics.is_pro && metrics.stored_bytes >= FREE_MAX_REPO_SIZE {
                println!("{} Storage limit reached mid-backup! Upgrade to Pro.", "❌".red());
                break;
            }
        }

        let recipe_entry = format!("{}\n", hash_str);
        recipe_file.write_all(recipe_entry.as_bytes()).await?;
    }

    pb.finish_and_clear();
    
    metrics.history.push(BackupRecord {
        timestamp: Utc::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        recipe: recipe_name.to_string(),
        size_mb: metadata.len() as f64 / (1024.0 * 1024.0),
    });
    
    save_metrics(&metrics).await?;
    
    let saved_mb = (dedup_chunks * BLOCK_SIZE) as f64 / (1024.0 * 1024.0);
    println!("{} Backup complete!", "✅".green().bold());
    
    let mut table = Table::new();
    table
        .set_header(vec![
            Cell::new("Metric").add_attribute(Attribute::Bold).fg(Color::Cyan),
            Cell::new("Value").add_attribute(Attribute::Bold).fg(Color::Cyan),
        ])
        .add_row(vec![
            Cell::new("New chunks"),
            Cell::new(new_chunks.to_string()).fg(Color::Yellow),
        ])
        .add_row(vec![
            Cell::new("Deduplicated chunks"),
            Cell::new(dedup_chunks.to_string()).fg(Color::Green),
        ])
        .add_row(vec![
            Cell::new("Space Saved"),
            Cell::new(format!("{:.2} MB", saved_mb)).add_attribute(Attribute::Bold).fg(Color::Green),
        ]);
    println!("\n{table}");

    Ok(())
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    // Print ASCII Banner
    println!("{}", r#"
    ____           __             ______            _          
   / __ \___  ____/ /_  ______   / ____/___  ____ _(_)___  ___ 
  / / / / _ \/ __  / / / / __ \ / __/ / __ \/ __ `/ / __ \/ _ \
 / /_/ /  __/ /_/ / /_/ / /_/ // /___/ / / / /_/ / / / / /  __/
/_____/\___/\__,_/\__,_/ .___//_____/_/ /_/\__, /_/_/ /_/\___/ 
                      /_/                 /____/               
"#.cyan().bold());

    let cli = Cli::parse();

    match cli.command {
        Commands::Auth { key } => {
            init_repo_if_needed().await?;
            let mut metrics = load_metrics().await;
            metrics.is_pro = true;
            save_metrics(&metrics).await?;
            println!("{} Pro License '{}' authenticated successfully!", "✅".green().bold(), key);
            println!("You now have access to unlimited storage, compression, background watching and advanced stats.");
        }
        Commands::Watch { file, compress } => {
            init_repo_if_needed().await?;
            let mut metrics = load_metrics().await;
            
            if !metrics.is_pro {
                println!("{} Watch feature requires Pro version.", "🔒".bright_red());
                return Ok(());
            }

            let path = Path::new(&file);
            if !path.exists() {
                println!("{} File not found: {}", "❌".red(), file);
                return Ok(());
            }

            let (tx, rx) = channel();
            let mut watcher = notify::recommended_watcher(tx).unwrap();
            watcher.watch(path, RecursiveMode::NonRecursive).unwrap();

            println!("{} Watching '{}' for changes in the background...", "👀".cyan().bold(), file.bold());
            println!("Press Ctrl+C to stop.");

            let mut last_backup = std::time::Instant::now();
            
            loop {
                if let Ok(Ok(event)) = rx.recv() {
                    if let notify::Event { kind: EventKind::Modify(_), .. } = event {
                        if last_backup.elapsed() > Duration::from_secs(2) {
                            let timestamp = Utc::now().format("%Y%m%d%H%M%S");
                            let recipe_name = format!("{}_{}", path.file_name().unwrap().to_string_lossy(), timestamp);
                            
                            // Re-load metrics to keep tracking up to date
                            metrics = load_metrics().await;
                            perform_backup(&file, &recipe_name, compress, &mut metrics).await?;
                            last_backup = std::time::Instant::now();
                            
                            println!("{} Watching '{}' for changes in the background...", "👀".cyan().bold(), file.bold());
                        }
                    }
                }
            }
        }
        Commands::Backup { file, compress } => {
            init_repo_if_needed().await?;
            let mut metrics = load_metrics().await;

            if compress && !metrics.is_pro {
                println!("{} Compression feature requires Pro version.", "🔒".bright_red());
                return Ok(());
            }

            let path = Path::new(&file);
            let recipe_name = path.file_name().unwrap().to_string_lossy().to_string();
            perform_backup(&file, &recipe_name, compress, &mut metrics).await?;
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
            println!("\n{} Restoring to '{}'...", "📥".cyan().bold(), destination.bold());

            let pb = ProgressBar::new(num_hashes as u64);
            pb.set_style(
                ProgressStyle::with_template(
                    "{spinner:.green} [{elapsed_precise}] [{wide_bar:.green/blue}] {pos}/{len} chunks ({eta})",
                )
                .unwrap()
                .progress_chars("=>-"),
            );

            for (i, hash) in hashes.iter().enumerate() {
                let chunk_path = repo_dir.join("chunks").join(hash);
                let zst_path = repo_dir.join("chunks").join(format!("{}.zst", hash));
                
                let mut chunk_data = if zst_path.exists() {
                    let compressed = fs::read(&zst_path).await?;
                    zstd::stream::decode_all(Cursor::new(compressed))?
                } else if chunk_path.exists() {
                    fs::read(&chunk_path).await?
                } else {
                    pb.finish_and_clear();
                    println!("{} Corrupted backup: Missing chunk {}", "❌".red().bold(), hash);
                    return Ok(());
                };

                if i == num_hashes - 1 {
                    while chunk_data.last() == Some(&0) {
                        chunk_data.pop();
                    }
                }

                dest_file.write_all(&chunk_data).await?;
                pb.inc(1);
            }

            pb.finish_and_clear();
            println!("{} Restore complete!", "✅".green().bold());
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

            let mut table = Table::new();
            table
                .set_header(vec![
                    Cell::new("Metric").add_attribute(Attribute::Bold).fg(Color::Cyan),
                    Cell::new("Value").add_attribute(Attribute::Bold).fg(Color::Cyan),
                ])
                .add_row(vec![
                    Cell::new("Repository size"),
                    Cell::new(format!("{:.0} MB / 1 GB", repo_mb)).fg(Color::Yellow),
                ])
                .add_row(vec![
                    Cell::new("Space saved"),
                    Cell::new(format!("{:.1} GB ({}%)", saved_gb, pct)).fg(Color::Green),
                ]);

            println!("\n{}", "📊 Repository Statistics".bold().cyan());
            println!("{table}");

            // Advanced Stats if PRO
            if metrics.is_pro {
                let usd_saved = saved_gb * 0.10; // Assuming $0.10 per GB average cloud egress/storage cost
                
                println!("\n{}", "💎 PRO Statistics".bold().magenta());
                println!("Financial Savings: ${:.2} USD (estimated at $0.10/GB)", usd_saved);
                
                if !metrics.history.is_empty() {
                    println!("\n{}", "📜 Backup History".bold().cyan());
                    let mut hist_table = Table::new();
                    hist_table.set_header(vec![
                        Cell::new("Timestamp").fg(Color::Cyan),
                        Cell::new("Recipe Name").fg(Color::Cyan),
                        Cell::new("Original Size").fg(Color::Cyan),
                    ]);
                    
                    // Show last 5 backups
                    for record in metrics.history.iter().rev().take(5) {
                        hist_table.add_row(vec![
                            Cell::new(&record.timestamp),
                            Cell::new(&record.recipe),
                            Cell::new(format!("{:.2} MB", record.size_mb)),
                        ]);
                    }
                    println!("{hist_table}");
                    println!("{}", "ℹ Note: Restoring specific historical versions is an Ultra-tier feature.".dimmed());
                }
            }

            // Warning at 80% capacity (800MB)
            if !metrics.is_pro {
                if repo_mb > 800.0 {
                    println!("\n{} {}", "⚠".yellow().bold(), "You are approaching the free plan limit".yellow());
                    println!("{}", "Upgrade to Pro for unlimited storage".italic());
                } else {
                    println!("\n{}", "✨ Upgrade to Pro for unlimited storage and compression".italic().dimmed());
                }
            }
        }
    }

    Ok(())
}
