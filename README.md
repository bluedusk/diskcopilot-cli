# diskcopilot-cli

Fast macOS disk scanner and query tool. Scans your filesystem, caches metadata in SQLite, and lets you query it instantly.

Designed for two consumers:
- **Humans** — via [Yazi](https://yazi-rs.github.io/) file manager plugin or CLI
- **AI agents** — via `--json` output on all query commands

## Performance

- **~12s** to scan a home directory with 1.4M files on APFS
- Uses macOS `getattrlistbulk(2)` for bulk metadata retrieval (3-6x faster than readdir+stat)
- Falls back to [jwalk](https://crates.io/crates/jwalk) on non-APFS volumes
- Size accuracy matches DaisyDisk/Finder (decimal units, APFS firmlink handling)

## Install

```bash
# One-line install (Apple Silicon)
curl -fsSL https://github.com/bluedusk/diskcopilot-cli/releases/latest/download/diskcopilot-cli-aarch64-apple-darwin.tar.gz | tar xz && sudo mv diskcopilot-cli /usr/local/bin/

# Intel Mac
curl -fsSL https://github.com/bluedusk/diskcopilot-cli/releases/latest/download/diskcopilot-cli-x86_64-apple-darwin.tar.gz | tar xz && sudo mv diskcopilot-cli /usr/local/bin/
```

Or build from source (requires Rust 1.70+):

```bash
git clone https://github.com/bluedusk/diskcopilot-cli
cd diskcopilot-cli && make install
```

## Usage

### Scan

```bash
diskcopilot-cli scan ~/Downloads              # files >= 1MB (default)
diskcopilot-cli scan ~ --full                 # all files
diskcopilot-cli scan / --force --full         # full drive (--force for system paths)
```

### Query

All query commands support `--json` for machine-readable output.

```bash
# Directory size tree
diskcopilot-cli query tree ~ --depth 2

# Largest files
diskcopilot-cli query large-files ~ --min-size 100M --limit 50

# Recently modified
diskcopilot-cli query recent ~ --days 3

# Old files
diskcopilot-cli query old ~ --days 180

# Dev artifacts (node_modules, target, .build, etc.)
diskcopilot-cli query dev-artifacts ~

# Files by extension
diskcopilot-cli query ext ~ --ext mp4

# Search by name
diskcopilot-cli query search ~ --name docker

# Cleanup summary report
diskcopilot-cli query summary ~

# Duplicate files (reads content for hashing — slower)
diskcopilot-cli query duplicates ~

# Scan metadata
diskcopilot-cli query info ~
```

### Delete

```bash
diskcopilot-cli delete /path/to/file --trash       # move to Trash
diskcopilot-cli delete /path/to/file --permanent    # permanent delete
```

### JSON output for AI agents

```bash
diskcopilot-cli query large-files ~ --json | jq '.[0]'
```

```json
{
  "name": "big-video.mp4",
  "full_path": "/Users/you/Downloads/big-video.mp4",
  "disk_size": 4200000000,
  "modified_at": 1710000000
}
```

## Yazi Plugin

A [Yazi](https://yazi-rs.github.io/) plugin that adds disk analytics to your file manager. See [diskcopilot.yazi/README.md](diskcopilot.yazi/README.md) for setup.

**Features:**
- `S` to scan current directory
- `d` + key for analytics (large files, duplicates, dev artifacts, etc.)
- Directory previewer showing size breakdown when hovering

```bash
make install-plugin    # install to ~/.config/yazi/plugins/
```

## Architecture

```
scan → SQLite cache → query / Yazi plugin / AI agent
```

- **Scanner** — two engines: `getattrlistbulk(2)` (macOS primary) and jwalk (fallback)
- **Cache** — SQLite at `~/.diskcopilot/cache/<blake3-hash>.db`, one DB per scan root
- **Query** — SQL queries against the cache, with pretty-print or JSON output
- **Yazi plugin** — Lua calling `diskcopilot-cli` via shell commands

See [docs/scanning-algorithm.md](docs/scanning-algorithm.md) for the scanner's evolution through 6 iterations.

## Privacy & Security

**diskcopilot-cli scans filesystem metadata only.** It does not read file contents, connect to the internet, or send data anywhere.

What it collects:
- File/directory names, sizes, timestamps, extensions
- Inode numbers (for hardlink dedup during scan)

What it does NOT do:
- Read file contents (exception: `query duplicates` reads content to compute blake3 hashes for dedup — hashes are stored locally only)
- Make any network connections — there are zero networking dependencies
- Send telemetry, analytics, or crash reports
- Access keychain, credentials, or sensitive system data

All data is stored locally in `~/.diskcopilot/cache/` and is readable only by your user account.

**Verify for yourself:** the project has no networking crates — check `Cargo.toml` or run `cargo tree | grep -i 'http\|reqwest\|hyper\|curl'`.

### Permissions

- Works without elevated permissions for your home directory
- Full Disk Access (System Settings > Privacy) is needed to scan system-wide
- `sudo` gives filesystem access but not the same results as Full Disk Access (SIP-protected paths remain inaccessible)
- System paths (`/System`, `/usr`, etc.) require `--force` flag as a safety measure

## Development

```bash
cargo build --release       # release build
cargo test                  # all tests
cargo clippy                # lint
cargo fmt                   # format
make check                  # fmt + lint + test
```

## License

MIT
