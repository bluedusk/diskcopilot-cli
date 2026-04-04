# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Test Commands

```bash
cargo build --release               # release build
cargo test                           # all tests (unit + integration)
cargo test --lib                     # unit tests only
cargo test scanner::walker::tests::test_scan_simple_tree  # single test
cargo clippy                         # lint
cargo fmt                            # format
make check                           # fmt + lint + test
make install                         # install to ~/.cargo/bin
```

## Usage

```bash
# Scan
diskcopilot-cli scan ~/Downloads              # default (files >= 1MB)
diskcopilot-cli scan ~ --full                 # all files
diskcopilot-cli scan / --force --full         # full drive (--force for system paths)

# Query
diskcopilot-cli query tree ~ --depth 2        # directory size tree
diskcopilot-cli query large-files ~           # largest files
diskcopilot-cli query recent ~ --days 3       # recently modified
diskcopilot-cli query old ~ --days 180        # old files
diskcopilot-cli query dev-artifacts ~         # node_modules, target, etc.
diskcopilot-cli query ext ~ --ext mp4         # files by extension
diskcopilot-cli query search ~ --name docker  # search by name
diskcopilot-cli query summary ~               # cleanup report
diskcopilot-cli query duplicates ~            # find duplicates (slow)
diskcopilot-cli query info ~                  # scan metadata
diskcopilot-cli query large-files ~ --json    # JSON output for AI agents
diskcopilot-cli query sql "SELECT ..." ~      # raw SQL against cache

# Delete
diskcopilot-cli delete /path --trash
diskcopilot-cli delete /path --permanent

# Interactive web UI
diskcopilot-cli serve ~                                    # open cleanup dashboard
diskcopilot-cli serve ~ --insights-file /tmp/insights.txt  # with AI analysis
diskcopilot-cli serve ~ --port 8080                        # custom port
```

## Architecture

**Pipeline:** scan → SQLite cache → query/Yazi plugin/AI agent. The binary name is `diskcopilot-cli`; the library crate is `diskcopilot`.

### Scanner (two engines)

- **`scanner/bulk_walker.rs`** (macOS primary) — Uses `getattrlistbulk(2)` for bulk metadata per directory. Parallel via `rayon::scope` + `crossbeam_channel`. 3-6x faster than jwalk on APFS. See [docs/scanning-algorithm.md](docs/scanning-algorithm.md) for detailed evolution and benchmarks.
- **`scanner/walker.rs`** (fallback) — Uses jwalk with `process_read_dir` for parallel stat. Falls back automatically on non-APFS volumes (FAT32, exFAT, NTFS).
- **`scanner/metadata.rs`** — `file_extension()` helper only (other metadata functions were removed).
- **`scanner/safety.rs`** — Dangerous path blocklist for delete operations (prefix + exact matching).

### Cache

- **`cache/schema.rs`** — `open_db()` (NORMAL sync for reads), `open_db_for_scan()` (sync OFF for bulk writes). Indexes deferred until after bulk insert.
- **`cache/writer.rs`** — `CacheWriter` with `begin()`/`commit()` for single-transaction writes. `finalize()` runs `compute_dir_sizes()` — sparse UPDATE via temp table (only updates dirs with files, not all 175k).
- **`cache/reader.rs`** — Query functions: `load_root`, `load_children`, `reconstruct_path(s)`, `query_large_files`, `query_recent_files`, `query_old_files`, `query_dev_artifacts`, `query_by_extension`, `query_by_name`, `query_summary`, `find_duplicates`, `load_scan_meta`, `load_tree_to_depth`. All output types derive `Serialize` for JSON.
- Cache path: `~/.diskcopilot/cache/<blake3-hash-of-canonical-path>.db`

### Other Modules

- **`output.rs`** — Pretty-print formatting for CLI output (tree with colored bars, file tables, summary report).
- **`delete/trash.rs`** — Safe deletion with `is_dangerous_path` guard. `DeleteResult` derives `Serialize`.
- **`format.rs`** — `format_size()` uses **decimal units** (1 GB = 10^9 bytes) to match macOS/Finder/DaisyDisk. `parse_size()` for parsing human sizes like "100M".
- **`server.rs`** — Axum web server for interactive cleanup UI. Serves HTML template + JSON API. Per-session auth token for trash operations, CORS locked to localhost.
- **`config/loader.rs`** — TOML config from `~/.diskcopilot/config.toml`.

### Yazi Plugin

- **`diskcopilot.yazi/`** — Yazi plugin calling `diskcopilot-cli` via `Command()`. Keybindings for scan/query, directory previewer, bundled `json.lua` parser. Install: `make install-plugin`.

## Key Design Decisions

- **PRAGMA journal_mode=OFF during scan** — cache is rebuildable, no journal needed. Set in main.rs before scan starts.
- **Empty dir pruning** — 97% of dirs on macOS are empty leaves. Only dirs with files (directly or transitively) are written to SQLite.
- **Sparse dir size rollup** — `compute_dir_sizes()` uses grouped aggregation + indexed temp table to UPDATE only dirs with files. Was 47s with correlated subqueries, now 0.01s.
- **APFS firmlink exclusions** — `/System/Volumes/Data` is excluded to prevent double-counting when scanning `/`.
- **Bundle skip** — `.app`, `.framework`, `.bundle`, `.plugin`, `.kext` are sized as opaque entries on parallel threads, not descended into.
- **Decimal GB** — All user-facing sizes use 1 GB = 1,000,000,000 bytes (matching macOS conventions).

## Agent Integration

- **Skills plugin** — [github.com/bluedusk/diskcopilot-skills](https://github.com/bluedusk/diskcopilot-skills). Claude Code plugin with `diskcopilot` skill and `/diskcopilot:scan`, `/diskcopilot:cleanup`, `/diskcopilot:report` commands. The skill auto-installs the CLI binary on first use.
- **`query sql`** — Agents write arbitrary read-only SQL against the cache (dirs, files, scan_meta tables). Primary query interface for AI agents.
- **`serve`** — Agents write analysis to a file, launch `serve --insights-file` for interactive browser-based cleanup. Auth token + CORS protect the trash API.

## Key Documents

- [docs/scanning-algorithm.md](docs/scanning-algorithm.md) — Detailed scanner evolution: 6 iterations, benchmarks, failures, and learnings.
