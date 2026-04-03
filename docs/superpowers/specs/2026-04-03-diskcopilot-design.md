# DiskCopilot CLI — Design Spec

## Overview

A Rust CLI tool for scanning Mac drives, visualizing disk usage in an interactive TUI, and enabling AI-driven disk cleanup workflows. Dual-mode interface: interactive TUI (like Yazi) for human use, structured JSON CLI for AI agent integration via a Claude Code plugin.

**Goal:** Help users find and clean up disk space, with AI assistance for evaluating what's safe to delete.

**Relationship to existing GUI (`x-scanner`):** A companion Tauri/React desktop app already exists at `../x-scanner/`. The CLI version shares the same domain but targets a different audience: terminal-native users and AI agents. Key differences:

| | GUI (x-scanner) | CLI (diskcopilot-cli) |
|---|---|---|
| Interface | Tauri + React desktop app | Ratatui TUI + structured CLI |
| AI integration | Browser-based lookup (limited) | First-class — AI agents call CLI directly |
| Cache | In-memory (15-min TTL) | SQLite on disk (persistent, queryable) |
| Distribution | Standalone app with licensing | Open CLI tool, Claude Code plugin |

**Reusable patterns from GUI (reference, not shared code):**
- Scanner architecture: 3-phase pipeline (parallel walk → sequential merge → assembly)
- `getattrlistbulk` macOS optimization for bulk metadata retrieval
- Inode dedup via `(device_id, inode)` pairs
- Bundle detection (.app, .framework, .bundle, .plugin, .kext)
- Dangerous path blocklist (77 patterns for safe deletion)
- Deep clean category presets (system caches, browser caches, dev artifacts)
- Physical size: `blocks * 512` for accurate APFS sizing

## CLI Interface

```
diskcopilot scan [OPTIONS] <PATH>
diskcopilot tui [OPTIONS] [PATH]
diskcopilot query [FILTERS] [PATH]
diskcopilot delete [OPTIONS] <FILES...>
diskcopilot diff [OPTIONS] [PATH]
```

### `scan` — Scan filesystem and cache results

```
diskcopilot scan <PATH>                     # default: cache dirs + files > 1MB
diskcopilot scan --full <PATH>              # cache every file, no size threshold
diskcopilot scan --dirs-only <PATH>         # cache directory aggregates only (~10MB)
diskcopilot scan --min-size 10M <PATH>      # cache dirs + files > 10MB
diskcopilot scan --snapshot <PATH>          # save as timestamped snapshot (not overwrite)
```

- Walks the filesystem from `<PATH>`, collects metadata per file/directory.
- **Parallel traversal** using `jwalk` thread pool for 3-5x speedup on SSDs.
- Deduplicates by inode (hard links counted once).
- Does not follow symlinks.
- Does not cross firmlinks by default (prevents double-counting macOS system volumes).
- Reports permission-denied directories in output rather than silently skipping.
- Shows a progress bar during scan (indicatif).
- Stores results in SQLite cache at `~/.diskcopilot/cache/`.

**Accuracy levels:**

```
diskcopilot scan <PATH>                     # default accuracy (fast)
diskcopilot scan --accurate <PATH>          # APFS clone detection + xattr measurement
diskcopilot scan --cross-firmlinks <PATH>   # follow firmlinks (macOS system volumes)
```

### `tui` — Interactive TUI

```
diskcopilot tui <PATH>                  # scan first, then launch TUI
diskcopilot tui --cached [PATH]         # launch from cached scan (no re-scan)
diskcopilot tui --depth <N> [PATH]      # limit display depth
diskcopilot tui --top <N> [PATH]        # show only N largest entries per dir
```

- Ratatui-based interactive tree view (similar polish level to Yazi).
- Navigate with arrow keys, expand/collapse directories.
- Sort by size, name, date.
- Delete files/directories with confirmation prompt.
- Shows both individual file sizes and cumulative directory sizes.
- Privacy masking OFF by default in TUI (local-only, human-facing).

### `query` — Structured queries (AI-facing)

All query commands output JSON for machine consumption.

```
diskcopilot query --largest <N> <PATH>
diskcopilot query --smallest <N> <PATH>
diskcopilot query --created-today <PATH>
diskcopilot query --created-after <DATE> <PATH>
diskcopilot query --modified-after <DATE> <PATH>
diskcopilot query --older-than <DURATION> <PATH>     # e.g. 30d, 1y
diskcopilot query --min-size <SIZE> <PATH>            # e.g. 100M, 1G
diskcopilot query --max-size <SIZE> <PATH>
diskcopilot query --pattern <REGEX> <PATH>            # path pattern match
diskcopilot query --ext <EXTENSIONS> <PATH>           # e.g. .log,.tmp,.bak
diskcopilot query --type cache <PATH>                 # built-in preset: caches
diskcopilot query --type dev-artifacts <PATH>         # node_modules, target/, .build/
diskcopilot query --type logs <PATH>                  # log files
```

- Filters are combinable: `--min-size 50M --older-than 90d`
- Queries run against the cached SQLite database (fast, no re-traversal).
- Requires a prior `scan`. Errors if no cache exists for the path.
- Privacy masking ON by default. Use `--no-mask` for unmasked output.

### `delete` — Safe deletion

```
diskcopilot delete --dry-run <FILE1> <FILE2> <DIR1>   # preview only
diskcopilot delete --confirm <FILE1> <FILE2> <DIR1>   # actually delete
```

- `--dry-run` always shows unmasked paths locally.
- `--confirm` requires explicit file paths. Confirms with the user before executing.
- Reports total space freed after deletion.

### `diff` — Compare snapshots (opt-in history)

```
diskcopilot diff <PATH>                          # latest vs most recent snapshot
diskcopilot diff --from <DATE> <PATH>            # latest vs specific snapshot
```

- Requires at least one prior `--snapshot` scan.
- Shows new files, deleted files, and size changes.

## Cache Architecture

### Storage

- Location: `~/.diskcopilot/cache/`
- Format: SQLite database
- One database per scanned root path (filename derived from hash of absolute path)
- Snapshots stored as separate databases with timestamp suffix

### Schema — Normalized Tree

```sql
CREATE TABLE scan_meta (
    id INTEGER PRIMARY KEY,
    root_path TEXT NOT NULL,
    scanned_at INTEGER NOT NULL,     -- unix timestamp
    total_files INTEGER,
    total_dirs INTEGER,
    total_size INTEGER,
    scan_duration_ms INTEGER
);

CREATE TABLE dirs (
    id INTEGER PRIMARY KEY,
    parent_id INTEGER REFERENCES dirs(id),
    name TEXT NOT NULL,               -- just the directory name, not full path
    file_count INTEGER,               -- direct children count
    total_file_count INTEGER,         -- recursive children count
    total_logical_size INTEGER,       -- recursive sum
    total_disk_size INTEGER,          -- recursive sum (actual blocks)
    created_at INTEGER,
    modified_at INTEGER
);

CREATE TABLE files (
    id INTEGER PRIMARY KEY,
    dir_id INTEGER NOT NULL REFERENCES dirs(id),
    name TEXT NOT NULL,               -- just the filename
    logical_size INTEGER NOT NULL,    -- st_size
    disk_size INTEGER NOT NULL,       -- st_blocks * 512
    created_at INTEGER,
    modified_at INTEGER,
    extension TEXT,
    inode INTEGER,                    -- for hard link dedup
    content_hash TEXT                 -- blake3 hash, populated on-demand for duplicate detection
);

-- Indexes for fast queries
CREATE INDEX idx_files_size ON files(disk_size DESC);
CREATE INDEX idx_files_created ON files(created_at);
CREATE INDEX idx_files_modified ON files(modified_at);
CREATE INDEX idx_files_extension ON files(extension);
CREATE INDEX idx_files_dir ON files(dir_id);
CREATE INDEX idx_dirs_parent ON dirs(parent_id);
CREATE INDEX idx_dirs_size ON dirs(total_disk_size DESC);
CREATE INDEX idx_files_hash ON files(content_hash) WHERE content_hash IS NOT NULL;
```

### Size Estimates (1TB drive, ~2M files)

| Mode | Cache Size |
|------|-----------|
| `--dirs-only` (dirs only) | ~10MB |
| Default (dirs + files > 1MB) | ~15-20MB |
| `--full` (everything) | ~150-200MB |

### Accuracy

Two accuracy levels to balance speed and precision:

#### Default (fast)

- **Hard links:** Deduplicated by inode via `HashSet<u64>`. Same file counted once.
- **Symlinks:** Not followed. Prevents double-counting and infinite loops.
- **Firmlinks:** Not crossed by default. macOS splits boot volume into System (read-only) and Data (writable) volumes connected by firmlinks. Crossing them double-counts. Detected via `getattrlist()` with `ATTR_CMN_FLAGS` (`SF_FIRMLINK`). Opt-in with `--cross-firmlinks`.
- **Sparse files:** Uses `st_blocks * 512` for actual disk usage, not `st_size`. A 1GB sparse file using 4KB on disk reports 4KB.
- **Permission-denied:** Reported in results with error flag, not silently skipped.
- **Two size fields:** `logical_size` (`st_size`, content length) and `disk_size` (`st_blocks * 512`, actual blocks). Directory totals roll up `disk_size`.

#### `--accurate` (slower, ~2x)

Adds these checks on top of the default:

- **APFS clones (copy-on-write):** When a file is duplicated on APFS, the clone shares physical disk blocks but both report full `st_blocks`. Without detection, disk usage is overcounted. Uses `getattrlist()` with `ATTR_CMNEXT_CLONEID` to detect clones sharing the same physical blocks. Clone groups are counted once.
- **Extended attributes / resource forks:** Uses `listxattr()` + `getxattr()` to measure xattr sizes. Checks `file/..namedfork/rsrc` for resource fork sizes. Adds these to the file's `disk_size`.

### Scan Performance

#### Parallel traversal

Uses `jwalk` for multi-threaded directory walking. On SSDs (all modern Macs), this is 3-5x faster than single-threaded `walkdir`.

| Files | Single-threaded | Parallel (jwalk) |
|-------|----------------|-------------------|
| 500K  | ~8s            | ~2-3s             |
| 2M    | ~30s           | ~6-8s             |
| 5M    | ~75s           | ~15-20s           |

#### SQLite write optimization

- **WAL mode:** `PRAGMA journal_mode=WAL` for concurrent read/write.
- **Synchronous OFF:** `PRAGMA synchronous=OFF` during scan. Safe — if scan crashes, just re-scan.
- **Batch inserts:** Buffer 5,000-10,000 entries per transaction. Single-row inserts would be 100x slower.
- **Prepared statements:** Reused across all inserts, no re-parsing.
- **Deferred index creation:** Build indexes after all data is inserted, not during.

#### Minimizing per-file overhead

- Single `lstat` syscall per file (provides size, timestamps, inode, blocks).
- No full path construction during traversal — store `(parent_dir_id, filename)`.
- Extension derived from filename on insert (cheap string operation).

## Privacy & Security

### Principles

1. No file content ever leaves the machine. Only metadata (paths, sizes, timestamps).
2. Path masking redacts sensitive segments before AI sees them.
3. Cache stored locally only, never uploaded.

### Path Masking

Configurable in `~/.diskcopilot/config.toml`:

```toml
[privacy]
mask_patterns = [
    "~/.ssh/*",
    "~/.aws/*",
    "~/.gnupg/*",
    "*/.env",
    "*/credentials*",
    "*/secret*",
]
```

**Behavior:**
- `query` commands: masking ON by default (AI-facing). Use `--no-mask` to disable.
- `tui` mode: masking OFF by default (human-facing, local only).
- Masked entries still show size, timestamps, extension — enough for AI to reason about cleanup without exposing sensitive names.
- `delete --dry-run` always shows unmasked paths locally.

### Delete Safety

- AI must reference exact file paths for deletion.
- `--dry-run` required before `--confirm` in AI workflows.
- Tool confirms with user before executing any deletion.

## Interactive TUI (v1 Priority)

### Visual Design

Yazi-level polish. The TUI should feel like a modern, well-crafted tool — not a throwback terminal app.

- **Ratatui + crossterm** backend.
- **Nerd Font icons** — file type icons (folder, code file, image, archive, etc.) using Nerd Font glyphs. Graceful fallback to Unicode symbols if Nerd Fonts aren't installed.
- **Color scheme** — rich, purposeful color palette:
  - Directories: bold blue
  - Large files (top 10% by size): red/orange to draw attention
  - Medium files: yellow
  - Small files: dim/gray
  - Selected item: highlighted with contrast background
  - Size bars: gradient from green (small) to red (large)
- **Theming** — built-in themes (dark, light, dracula, catppuccin) selectable via config or `--theme` flag. Custom themes via `~/.diskcopilot/themes/`.
- **Layout:**
  - Top bar: breadcrumb path navigation + view tabs
  - Left panel: tree view with expand/collapse (or list view depending on active tab)
  - Right panel: details pane (selected item info, size breakdown, timestamps)
  - Bottom bar: status line (total scanned, current path, sort mode)

### Views (Tab Switching)

Six views, all querying the same cached SQLite data. `Tab` / `Shift+Tab` to cycle.

```
[Tree]  [Large Files]  [Recent]  [Old]  [Dev Artifacts]  [Duplicates]
```

**Tree** (default) — hierarchical tree view with expand/collapse, cumulative directory sizes.

**Large Files** — flat list of files above a configurable threshold (default 500MB), sorted by size descending. Query: `WHERE disk_size > threshold ORDER BY disk_size DESC`.

**Recent** — files created or modified in the last 7 days (configurable), sorted by date. Query: `WHERE modified_at > now - 7d ORDER BY modified_at DESC`. Useful for finding unexpected recent disk growth.

**Old** — files not modified in over 1 year (configurable), sorted by size descending. Query: `WHERE modified_at < now - 1y ORDER BY disk_size DESC`. Largest stale files first — prime cleanup candidates.

**Dev Artifacts** — known safe-to-delete developer directories: `node_modules`, `target/`, `.next/`, `__pycache__/`, `.build/`, `Pods/`, `.gradle/`, etc. Query: `WHERE name IN (preset list)`. Shows size per artifact with total at top.

**Duplicates** — files with identical sizes, confirmed by content hash. Unlike other views, this requires an **on-demand background scan** — user triggers it, TUI shows progress, view populates when hashing completes. Only hashes files that share the same size (funnel approach: group by size → hash candidates). Not part of initial scan.

All views support the same actions (delete, mark, sort) and the detail pane updates per selection.

### Key Bindings

Vim-style navigation as primary, arrow keys as alternative. Designed to feel natural for terminal-native users.

**Navigation:**

| Key | Action |
|-----|--------|
| `j` / `Down` | Move cursor down |
| `k` / `Up` | Move cursor up |
| `l` / `Right` / `Enter` | Expand directory / enter |
| `h` / `Left` | Collapse directory / go to parent |
| `g` / `Home` | Jump to top |
| `G` / `End` | Jump to bottom |
| `Ctrl+d` | Page down |
| `Ctrl+u` | Page up |

**Actions:**

| Key | Action |
|-----|--------|
| `d` | Delete selected (confirmation dialog) |
| `D` | Delete selected (skip to trash instead of permanent) |
| `Space` | Toggle mark (multi-select for batch operations) |
| `v` | Invert marks |
| `a` | Mark all in current directory |

**View controls:**

| Key | Action |
|-----|--------|
| `s` | Cycle sort: size (desc) → name → date modified → date created |
| `S` | Reverse current sort order |
| `/` | Filter/search (fuzzy match on filename) |
| `Esc` | Clear filter / cancel dialog |
| `t` | Toggle top-N mode (show only largest entries) |
| `1`-`9` | Set depth limit (1-9 levels) |
| `0` | Remove depth limit |
| `i` | Toggle detail pane |
| `p` | Toggle percentage bars |

**Global:**

| Key | Action |
|-----|--------|
| `q` | Quit |
| `?` | Show help overlay with all keybindings |
| `r` | Refresh (re-scan current directory) |
| `Tab` | Next view (Tree → Large Files → Recent → ...) |
| `Shift+Tab` | Previous view |

### Display Elements

- **File sizes:** Human-readable (1.2 GB, 340 MB, 4.5 KB). Right-aligned column.
- **Cumulative directory sizes:** Shown inline next to directory name. Calculated from cache.
- **Percentage bars:** Horizontal bar next to each entry showing proportion of parent directory size. Gradient color (green → yellow → red).
- **Item count:** Directories show child count in parentheses, e.g. `src/ (42 items)`.
- **File type icons:** Nerd Font glyphs per file type:
  - 󰉋 Folder, 󰉋 Folder (open)
  -  Rust,  Python,  JavaScript,  TypeScript
  -  Image,  Video,  Audio
  -  Archive (zip, tar, gz)
  -  Config/dotfiles
  -  Generic file
- **Confirmation dialogs:** Modal overlay for destructive actions. Shows item name, size, and "Are you sure?" with `y/n` keybinding. For batch deletes, shows count and total size.
- **Progress indicator:** During scan, shows files/s counter, elapsed time, and spinner.

## TUI Architecture (from Yazi)

### Async Event Loop

Single-threaded event dispatch with exclusive state access (no locks needed for app state):

```
loop {
    select! {
        timeout => render if NEED_RENDER flag set (10ms debounce)
        events  => batch up to 50 events, dispatch sequentially
    }
}
```

- **Atomic render flag** (`AtomicU8`): 0 = no render, 1 = full, 2 = partial (progress only)
- **Synchronized terminal updates** via crossterm `BeginSynchronizedUpdate` (prevents flicker)
- **Background scan** runs in tokio task, emits progress events via global channel

### Event System

```rust
enum Event {
    Key(KeyEvent),
    Resize,
    ScanProgress { files: usize, total_size: u64 },
    ScanComplete,
    Render(bool),  // partial flag
}
```

Global emit channel (`emit!(Event::...)`) allows background tasks to notify UI.

### State Separation

Following ratatui's `StatefulWidget` pattern — state is separate from rendering:

```rust
struct App {
    tree_state: TreeState,      // cursor, expanded nodes, scroll offset
    sort_mode: SortMode,
    filter: Option<String>,
    scan_progress: Option<ScanProgress>,
    confirm_dialog: Option<ConfirmDialog>,
    // ... no rendering logic here
}
```

### Virtualized Rendering

Only render visible tree nodes. For a tree with 100K entries but 40 visible rows, only 40 items are processed per frame. The `tui-tree-widget` handles this via `TreeState` offset tracking.

## Claude Code Plugin

The project ships with a Claude Code plugin so AI agents can use `diskcopilot` via natural language.

### Plugin Structure

```
plugin/
  plugin.json
  skills/
    disk-scan.md       # teaches AI to invoke diskcopilot scan
    disk-query.md      # teaches AI to compose query commands
    disk-clean.md      # teaches AI safe deletion workflow
    disk-view.md       # teaches AI to launch diskcopilot tui
```

### Skills

**`disk-scan`** — Triggers on "scan my disk", "check disk usage", "how much space"
- Guides AI to run `diskcopilot scan` with appropriate flags.
- Distinguishes full vs quick scan based on user intent.

**`disk-query`** — Triggers on "find largest files", "what's taking up space", "show caches"
- Maps natural language to `diskcopilot query` filter flags.
- Awareness of privacy masking (agent knows paths may be redacted).

**`disk-clean`** — Triggers on "clean up", "delete junk", "free up space"
- Enforces safe workflow: query first → present findings → dry-run → confirm.
- AI explains what it plans to delete and why it's safe before executing.
- Never deletes without user confirmation.

**`disk-view`** — Triggers on "show me the results", "open disk viewer"
- Launches `diskcopilot tui` for the user to browse interactively.

### Example Workflows

```
User: "Scan my home folder and tell me what's taking up space"
Agent: diskcopilot scan ~ → diskcopilot query --largest 20 ~
       → presents categorized breakdown

User: "Find all node_modules and caches I can safely delete"
Agent: diskcopilot query --type dev-artifacts --type cache ~
       → presents list with sizes, explains safety

User: "Delete them"
Agent: diskcopilot delete --dry-run <paths>
       → shows preview, asks confirmation
       → diskcopilot delete --confirm <paths>
```

## Project Structure

```
diskcopilot-cli/
├── src/
│   ├── main.rs              # CLI entry point (clap), async runtime setup
│   ├── scanner/             # filesystem traversal, metadata collection
│   │   ├── mod.rs
│   │   ├── walker.rs        # jwalk parallel walking, inode dedup (DashSet)
│   │   ├── metadata.rs      # file metadata extraction (macOS-specific)
│   │   └── safety.rs        # dangerous path blocklist (from x-scanner patterns)
│   ├── cache/               # SQLite storage
│   │   ├── mod.rs
│   │   ├── schema.rs        # table definitions, migrations
│   │   ├── writer.rs        # bulk insert with batched transactions
│   │   └── reader.rs        # query execution, tree reconstruction
│   ├── tui/                 # Ratatui interactive UI (Yazi-inspired architecture)
│   │   ├── mod.rs
│   │   ├── app.rs           # App state + async event loop (batched, debounced)
│   │   ├── event.rs         # Event enum, global channel (emit! macro)
│   │   ├── dispatcher.rs    # Event routing, layer-aware dispatch
│   │   ├── tree.rs          # Tree widget (tui-tree-widget + size columns)
│   │   ├── detail.rs        # Detail pane (selected item info)
│   │   ├── search.rs        # Fuzzy search with nucleo
│   │   ├── confirm.rs       # Modal confirmation dialogs
│   │   ├── theme.rs         # Theme loading (RoCell), built-in + custom themes
│   │   ├── icons.rs         # Nerd Font icon mapping by extension/type
│   │   └── keymap.rs        # Vim-style keybind config + chord handling
│   ├── delete/              # safe deletion
│   │   ├── mod.rs
│   │   └── trash.rs         # Move to macOS Trash (recoverable)
│   └── config/              # configuration
│       ├── mod.rs
│       └── loader.rs        # TOML config loading + defaults
├── themes/                  # built-in theme files
│   ├── dark.toml
│   ├── light.toml
│   ├── dracula.toml
│   └── catppuccin.toml
├── plugin/                  # Claude Code plugin (v2)
│   ├── plugin.json
│   └── skills/
│       ├── disk-scan.md
│       ├── disk-query.md
│       ├── disk-clean.md
│       └── disk-view.md
├── Cargo.toml
└── README.md
```

## Open Source References

Patterns and learnings borrowed from proven Rust CLI tools:

| Tool | What we borrow |
|------|---------------|
| **Yazi** | Async event loop (batched events, debounced render, atomic render flags), virtualized rendering (only visible nodes), vim keybindings with chord system, TOML theming with `RoCell`, Nerd Font icon mapping |
| **fd** | Thread pool constraints (cap at ~64 threads for startup perf), `ignore` crate for gitignore-aware traversal |
| **dust** | Smart tree truncation (top-N, dive into large dirs), hierarchical color coding with shades of grey |
| **dua-cli** | Keep tree rendering simple (they removed over-engineered abstractions — simplicity wins), parallel scan with real-time sorting |
| **broot** | Non-blocking fuzzy search (keystroke interrupts current search), background size computation |
| **ncdu** | Single-letter mnemonics (d/s/n/g — proven UX), three scan verbosity levels |
| **lsd** | Icon mapping by extension/type/filename pattern, LS_COLORS compatibility |
| **x-scanner (GUI)** | 3-phase scan pipeline, `getattrlistbulk` macOS optimization (v2), inode dedup via `(device_id, inode)`, bundle detection, 77 dangerous path patterns, deep clean category presets |

## Dependencies

### Core

| Crate | Purpose |
|-------|---------|
| `clap` | CLI argument parsing with derive |
| `ratatui` | TUI framework |
| `crossterm` | Terminal backend for ratatui |
| `tui-tree-widget` | Tree rendering widget (StatefulWidget pattern, proven with ratatui) |
| `rusqlite` | SQLite storage (with `bundled` feature) |
| `serde` + `serde_json` | JSON serialization for query output and config |
| `toml` | Config file parsing (themes, keybinds) |
| `dirs` | Platform-standard paths (~/.diskcopilot/) |

### Scanning

| Crate | Purpose |
|-------|---------|
| `jwalk` | Parallel filesystem traversal (rayon-based, 3-5x over single-threaded) |
| `ignore` | gitignore-aware walking (from fd/ripgrep), optional skip of .git/node_modules |
| `xattr` | macOS extended attribute measurement (for `--accurate` mode) |

### TUI Extras

| Crate | Purpose |
|-------|---------|
| `nucleo` | Fuzzy search engine (from Helix editor, 6x faster than skim, Unicode-aware) |
| `indicatif` | Progress bar during scan phase |

### Performance (add if profiling shows need)

| Crate | Purpose |
|-------|---------|
| `parking_lot` | Faster mutexes/RwLocks under contention |
| `dashmap` | Lock-free concurrent hashmaps for inode dedup (used by x-scanner GUI) |

## v1 Priorities

1. **Scanner** — fast (parallel jwalk), accurate (inode dedup, disk_size, firmlink-aware, optional APFS clone detection)
2. **TUI** — polished visuals (colors, icons, themes), excellent keyboard interaction (vim-style, multi-select, sort, filter)

The following are v1 deliverables but lower priority than scanner + TUI:
- `scan` command with caching
- `tui` command with all display/interaction features
- `delete` from within TUI (with confirmation)
- Basic config file (`~/.diskcopilot/config.toml`) for themes

## v2 Features

- **CLI query interface** — `diskcopilot query` with JSON output for AI agent integration
- **Claude Code plugin** — skills for disk-scan, disk-query, disk-clean, disk-view
- **Privacy masking** — path redaction for AI-facing output
- **CLI delete command** — `diskcopilot delete --dry-run / --confirm` (separate from TUI delete)
- **Snapshot & diff** — `--snapshot` flag and `diskcopilot diff` for comparing scans over time
- **Network drive / NAS support** — scanning mounted SMB/NFS/AFP volumes (e.g. `/Volumes/MyNAS/`). Needs special handling: much slower traversal (network-bound), unreliable `st_blocks` on non-APFS filesystems, lower timestamp precision. Should skip `--accurate` mode on non-APFS volumes.
- Real-time filesystem watching
- Cloud storage integration
- GUI (desktop app)
