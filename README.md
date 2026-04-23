# 🚀 Dedup CLI

**Reduce your storage usage by storing only what actually changed.**

Dedup CLI is a fast local backup tool built in Rust that removes duplicate data at the block level.

Instead of saving full files over and over again, it stores only new blocks, which makes it especially useful for logs, dumps, and files that grow over time.

Everything runs locally, so your data never leaves your machine.

## 📦 Features

- **Save storage space:** Only new blocks are stored.
- **Local-first:** All data stays on your machine in `~/.dedup`.
- **Fast backups:** Built in Rust for performance.
- **Exact restore:** Rebuild files byte-for-byte from lightweight recipe files.
- **Works well with growing files:** Especially useful for logs, dumps, and repeated backups.

## 🎯 Who is this for?

Dedup CLI is a great fit for:

- **DevOps engineers** managing logs and backup growth
- **Backend developers** working with database dumps or repeated exports
- **Developers with duplicate files** across multiple folders or projects
- **Anyone building local backup workflows** without relying on cloud infrastructure
- **People who want privacy-first tools** that keep all data on their own machine

If your files change gradually over time, Dedup CLI can help you store far less data without making your workflow more complicated.

## 🛠 Installation

You can install Dedup CLI locally on macOS, Linux, or Windows using Cargo. This avoids committing build artifacts and does not require copying the `target/` folder into the repository.

### Requirements

Install Rust and Cargo first:

- **macOS / Linux:** `curl https://sh.rustup.rs -sSf | sh`
- **Windows:** install `rustup-init.exe` from [rustup.rs](https://rustup.rs)

Restart your terminal after installation, then verify:

```bash
rustc --version
cargo --version
```

### Option 1: Install the CLI locally with Cargo

From the project root, run:

```bash
cargo install --path .
```

This installs the binary as `dedup-engine`.

Verify the installation:

```bash
dedup-engine --help
```

### PATH notes by operating system

Cargo installs binaries into its local bin directory. If `dedup-engine` is not found after installation, add the correct directory to your `PATH`.

#### macOS and Linux

Cargo usually installs to:

```bash
~/.cargo/bin
```

Add this to your shell profile such as `~/.bashrc`, `~/.zshrc`, or `~/.profile`:

```bash
export PATH="$HOME/.cargo/bin:$PATH"
```

Reload your shell:

```bash
source ~/.bashrc
```

If you use `zsh`, reload `~/.zshrc` instead.

#### Windows

Cargo usually installs binaries to:

```powershell
$env:USERPROFILE\.cargo\bin
```

If needed, add that directory to your user `Path` environment variable, then reopen PowerShell or Command Prompt.

You can verify with:

```powershell
dedup-engine.exe --help
```

### Option 2: Run without installing

If you only want to test the CLI from source:

```bash
cargo run -- backup my_file.txt
```

### Option 3: Using Docker

Build the image:

```bash
docker build -t dedup-engine .
```

Run the container (mounting your local directory to process files and a persistent volume for the repository):

```bash
docker run --rm -v $(pwd):/workspace -v dedup_repo:/home/dedupuser/.dedup-engine -w /workspace dedup-engine backup my_file.txt
```

## 💻 Usage

### 1. Backup a file
The first time you run this, it will automatically initialize the repository at `~/.dedup`.

```bash
dedup-engine backup database_dump.sql
```

### 2. View your storage stats
Check how much space you have saved through deduplication.

```bash
dedup-engine stats
```

### 3. Restore a file
Rebuild your original file from its recipe.

```bash
dedup-engine restore database_dump.sql ./restored_db.sql
```

## 📊 Example Output

```text
Original size:     120 MB
Stored size:       6.2 MB
Space saved:       94.8%

Chunks:
- New:     210
- Reused:  3840
```

## 💎 Free Tier Limits

Dedup CLI Community Edition is completely free forever with the following limits perfectly suited for personal use:

- **Up to 300MB** per single file backup.
- **Up to 1GB** total repository storage.
- **Unlimited backups** as long as storage permits.
- **Local usage only**.

*Need to backup multi-gigabyte databases or compress your backups? Run `dedup-engine backup file.txt --compress` to see how to upgrade to the Pro version!*
