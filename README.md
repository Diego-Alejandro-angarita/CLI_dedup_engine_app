# 🚀 Dedup CLI

**Dedup CLI** is a blazingly fast, block-level deduplication backup tool built in Rust. It runs locally, ensuring complete data privacy while drastically reducing your storage footprint.

It works by splitting your files into microscopic 4KB blocks and calculating a lightning-fast cryptographic hash (FNV-1a 64-bit) for each block. It only stores the blocks it hasn't seen before.

## 📦 Features

- **Block-Level Deduplication:** Never save the same data twice.
- **Privacy First:** Everything runs and is stored locally in your `~/.dedup` directory. No cloud API required.
- **Lightning Fast:** Built in Rust with asynchronous I/O.
- **Easy Recovery:** Rebuild files byte-for-byte from lightweight `.recipe` files.

## 🛠 Installation

You can run Dedup CLI via Docker or by building it from source.

### Option 1: Using Docker (Recommended for testing)

Build the image:
```bash
docker build -t dedup-engine .
```

Run the container (mounting your local directory to process files and a persistent volume for the repository):
```bash
docker run --rm -v $(pwd):/workspace -v dedup_repo:/home/dedupuser/.dedup-engine -w /workspace dedup-engine backup my_file.txt
```

### Option 2: Build from Source

Ensure you have Rust and Cargo installed, then run:
```bash
cargo build --release
# Move the binary to your path
sudo mv target/release/dedup-engine /usr/local/bin/
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

## 💎 Free Tier Limits

Dedup CLI Community Edition is completely free forever with the following limits perfectly suited for personal use:

- **Up to 300MB** per single file backup.
- **Up to 1GB** total repository storage.
- **Unlimited backups** as long as storage permits.
- **Local usage only**.

*Need to backup multi-gigabyte databases or compress your backups? Run `dedup-engine backup file.txt --compress` to see how to upgrade to the Pro version!*
