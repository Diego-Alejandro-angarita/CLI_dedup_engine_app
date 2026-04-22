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
        file: Option<String>,
        /// Compress the backup (Pro/Team feature)
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
    /// Authenticate Pro or Team license
    Auth {
        /// License Key (starts with PRO- or TEAM-)
        key: String,
    },
    /// Watch a file and auto-backup changes (Pro/Team feature)
    Watch {
        /// File to watch
        file: Option<String>,
        /// Compress the backups
        #[arg(long)]
        compress: bool,
    },
    /// View backup history (Team feature)
    History,
}

#[derive(Serialize, Deserialize, Default, Clone)]
struct BackupRecord {
    timestamp: String,
    recipe: String,
    size_mb: f64,
}

#[derive(Serialize, Deserialize, Default, Clone)]
struct License {
    tier: String, // "free", "pro", "team"
}

#[derive(Serialize, Deserialize, Default, Clone)]
struct RepoMetrics {
    logical_bytes: u64,
    stored_bytes: u64,
    history: Vec<BackupRecord>,
}

#[derive(Serialize, Deserialize, Default, Clone)]
struct ProjectConfig {
    compression: Option<bool>,
    watch: Option<bool>,
    path: Option<String>,
    target: Option<String>,
}

fn compute_chunk_hash(buffer: &[u8]) -> String {
    let mut hash = FNV_OFFSET_BASIS;
    for &byte in buffer {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    format!("{:016x}", hash)
}

fn get_global_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".dedup")
}

async fn load_license() -> License {
    let global_dir = get_global_dir();
    let license_path = global_dir.join("license.json");
    if license_path.exists() {
        if let Ok(data) = fs::read_to_string(&license_path).await {
            if let Ok(lic) = serde_json::from_str(&data) {
                return lic;
            }
        }
    }
    License { tier: "free".to_string() }
}

async fn save_license(license: &License) -> std::io::Result<()> {
    let global_dir = get_global_dir();
    if !global_dir.exists() {
        fs::create_dir_all(&global_dir).await?;
    }
    let json = serde_json::to_string_pretty(license).unwrap();
    fs::write(global_dir.join("license.json"), json).await
}

async fn load_project_config() -> Option<ProjectConfig> {
    let config_path = Path::new("dedup.json");
    if config_path.exists() {
        if let Ok(data) = fs::read_to_string(config_path).await {
            if let Ok(config) = serde_json::from_str(&data) {
                return Some(config);
            }
        }
    }
    None
}

async fn get_repo_dir() -> PathBuf {
    if let Some(config) = load_project_config().await {
        if let Some(path) = config.path {
            return PathBuf::from(path);
        }
    }
    get_global_dir()
}

async fn init_repo_if_needed(repo_dir: &PathBuf) -> std::io::Result<()> {
    if !repo_dir.exists() {
        println!("{}", format!("No repository found at {:?}. Initializing...", repo_dir).yellow());
        fs::create_dir_all(repo_dir.join("chunks")).await?;
        fs::create_dir_all(repo_dir.join("recipes")).await?;
        
        let initial_metrics = RepoMetrics::default();
        let metrics_json = serde_json::to_string_pretty(&initial_metrics).unwrap();
        fs::write(repo_dir.join("metrics.json"), metrics_json).await?;
        
        println!("{} Repository created at {:?}", "✔".green(), repo_dir);
    }
    Ok(())
}

async fn load_metrics(repo_dir: &PathBuf) -> RepoMetrics {
    let path = repo_dir.join("metrics.json");
    if path.exists() {
        if let Ok(data) = fs::read_to_string(&path).await {
            if let Ok(metrics) = serde_json::from_str(&data) {
                return metrics;
            }
        }
    }
    RepoMetrics::default()
}

async fn save_metrics(repo_dir: &PathBuf, metrics: &RepoMetrics) -> std::io::Result<()> {
    let path = repo_dir.join("metrics.json");
    let json = serde_json::to_string_pretty(metrics).unwrap();
    fs::write(path, json).await
}

async fn perform_backup(
    file: &str, 
    recipe_name: &str, 
    compress: bool, 
    metrics: &mut RepoMetrics,
    repo_dir: &PathBuf,
    tier: &str
) -> std::io::Result<()> {
    let path = Path::new(file);
    if !path.exists() {
        println!("{} File not found: {}", "❌".red(), file);
        return Ok(());
    }

    let metadata = fs::metadata(&path).await?;
    let is_free = tier == "free";
    
    if is_free && metadata.len() > FREE_MAX_FILE_SIZE {
        println!(
            "{} File too large ({} MB). Free plan is limited to 300MB per file. Upgrade to Pro/Team.",
            "❌".red(),
            metadata.len() / (1024 * 1024)
        );
        return Ok(());
    }

    if is_free && metrics.stored_bytes >= FREE_MAX_REPO_SIZE {
        println!("{} Storage limit reached (1GB). Upgrade to Pro/Team for unlimited storage.", "❌".red());
        return Ok(());
    }

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
            
            if is_free && metrics.stored_bytes >= FREE_MAX_REPO_SIZE {
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
    
    save_metrics(repo_dir, &metrics).await?;
    
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
    let license = load_license().await;
    let config = load_project_config().await;

    // Reject non-team users trying to use custom repos (dedup.json path)
    if let Some(ref cfg) = config {
        if cfg.path.is_some() && license.tier != "team" {
            println!("{} Custom repository isolation requires the Team plan.", "🔒".bright_red());
            println!("Please authenticate with a Team license.");
            return Ok(());
        }
    }

    match cli.command {
        Commands::Auth { key } => {
            let mut new_lic = load_license().await;
            if key.starts_with("TEAM-") {
                new_lic.tier = "team".to_string();
                println!("{} Team License '{}' authenticated successfully!", "✅".green().bold(), key);
                println!("You have access to isolated repos, history, compression, and watch mode.");
            } else if key.starts_with("PRO-") {
                new_lic.tier = "pro".to_string();
                println!("{} Pro License '{}' authenticated successfully!", "✅".green().bold(), key);
                println!("You have access to unlimited storage, compression, and watch mode.");
            } else {
                println!("{} Invalid license key format.", "❌".red());
                return Ok(());
            }
            save_license(&new_lic).await?;
        }
        Commands::Watch { file, compress } => {
            if license.tier == "free" {
                println!("{} Watch feature requires Pro/Team version.", "🔒".bright_red());
                return Ok(());
            }

            let target_file = file
                .or_else(|| config.as_ref().and_then(|c| c.target.clone()))
                .expect("Must specify a file to watch or have a 'target' in dedup.json");

            let use_compress = compress || config.as_ref().and_then(|c| c.compression).unwrap_or(false);
            
            let repo_dir = get_repo_dir().await;
            init_repo_if_needed(&repo_dir).await?;
            let mut metrics = load_metrics(&repo_dir).await;

            let path = Path::new(&target_file);
            if !path.exists() {
                println!("{} File not found: {}", "❌".red(), target_file);
                return Ok(());
            }

            let (tx, rx) = channel();
            let mut watcher = notify::recommended_watcher(tx).unwrap();
            watcher.watch(path, RecursiveMode::NonRecursive).unwrap();

            println!("{} Watching '{}' for changes in the background...", "👀".cyan().bold(), target_file.bold());
            println!("Press Ctrl+C to stop.");

            let mut last_backup = std::time::Instant::now();
            
            loop {
                if let Ok(Ok(event)) = rx.recv() {
                    if let notify::Event { kind: EventKind::Modify(_), .. } = event {
                        if last_backup.elapsed() > Duration::from_secs(2) {
                            let timestamp = Utc::now().format("%Y%m%d_%H%M%S");
                            let recipe_name = format!("{}_{}", path.file_name().unwrap().to_string_lossy(), timestamp);
                            
                            metrics = load_metrics(&repo_dir).await;
                            perform_backup(&target_file, &recipe_name, use_compress, &mut metrics, &repo_dir, &license.tier).await?;
                            last_backup = std::time::Instant::now();
                            
                            println!("{} Watching '{}' for changes in the background...", "👀".cyan().bold(), target_file.bold());
                        }
                    }
                }
            }
        }
        Commands::Backup { file, compress } => {
            let use_compress = compress || config.as_ref().and_then(|c| c.compression).unwrap_or(false);
            if use_compress && license.tier == "free" {
                println!("{} Compression feature requires Pro/Team version.", "🔒".bright_red());
                return Ok(());
            }

            let target_file = file
                .or_else(|| config.as_ref().and_then(|c| c.target.clone()))
                .expect("Must specify a file to backup or have a 'target' in dedup.json");

            let repo_dir = get_repo_dir().await;
            init_repo_if_needed(&repo_dir).await?;
            let mut metrics = load_metrics(&repo_dir).await;

            let path = Path::new(&target_file);
            let timestamp = Utc::now().format("%Y%m%d_%H%M%S");
            let recipe_name = format!("{}_{}", path.file_name().unwrap().to_string_lossy(), timestamp);
            
            perform_backup(&target_file, &recipe_name, use_compress, &mut metrics, &repo_dir, &license.tier).await?;
        }
        Commands::Restore { recipe, destination } => {
            let repo_dir = get_repo_dir().await;
            init_repo_if_needed(&repo_dir).await?;
            
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
        Commands::History => {
            if license.tier != "team" {
                println!("{} History view requires the Team plan.", "🔒".bright_red());
                return Ok(());
            }

            let repo_dir = get_repo_dir().await;
            init_repo_if_needed(&repo_dir).await?;
            let metrics = load_metrics(&repo_dir).await;

            println!("\n{}", "📜 Backup History".bold().cyan());
            if metrics.history.is_empty() {
                println!("No backups found in this repository.");
                return Ok(());
            }

            let mut hist_table = Table::new();
            hist_table.set_header(vec![
                Cell::new("Timestamp").fg(Color::Cyan),
                Cell::new("Recipe Name").fg(Color::Cyan),
                Cell::new("Original Size").fg(Color::Cyan),
            ]);
            
            for record in metrics.history.iter().rev() {
                hist_table.add_row(vec![
                    Cell::new(&record.timestamp),
                    Cell::new(&record.recipe),
                    Cell::new(format!("{:.2} MB", record.size_mb)),
                ]);
            }
            println!("{hist_table}");
            println!("\nUse `dedup-engine restore <Recipe Name> <dest>` to retrieve a version.");
        }
        Commands::Stats => {
            let repo_dir = get_repo_dir().await;
            init_repo_if_needed(&repo_dir).await?;
            let metrics = load_metrics(&repo_dir).await;
            
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
                    Cell::new("Active Repository"),
                    Cell::new(repo_dir.to_string_lossy()).fg(Color::White),
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

            if license.tier != "free" {
                let usd_saved = saved_gb * 0.10; 
                println!("\n{}", "💎 Premium Statistics".bold().magenta());
                println!("Financial Savings: ${:.2} USD (estimated at $0.10/GB)", usd_saved);
            } else if repo_mb > 800.0 {
                println!("\n{} {}", "⚠".yellow().bold(), "You are approaching the free plan limit".yellow());
                println!("{}", "Upgrade to Pro/Team for unlimited storage".italic());
            } else {
                println!("\n{}", "✨ Upgrade to Pro/Team for unlimited storage and compression".italic().dimmed());
            }
        }
    }

    Ok(())
}
