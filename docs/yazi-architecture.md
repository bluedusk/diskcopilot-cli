# Yazi — Architecture & Tech Stack Reference

> Terminal file manager written in Rust. Non-blocking async I/O, Lua-extensible.
> Source: https://github.com/sxyazi/yazi

## Tech Stack

| Layer              | Technology                                    |
| ------------------ | --------------------------------------------- |
| Language           | Rust (Edition 2024, MSRV 1.92.0)              |
| Async Runtime      | Tokio 1.50 (full features)                    |
| Terminal UI        | Ratatui 0.30 + Crossterm 0.29                 |
| Plugin Engine      | mlua 0.11.6 (vendored Lua 5.5)               |
| Config Format      | TOML                                          |
| Allocator          | jemalloc (Linux via tikv-jemallocator 0.6.1)  |
| SSH/SFTP           | russh 0.59 (pure Rust)                        |
| Image Previews     | Kitty, iTerm2/Sixel, Chafa, Uberzug++         |
| Syntax Highlight   | Syntect 5.3                                   |
| File Watching      | notify 8.2                                    |
| Hashing            | xxhash3 (twox-hash 2.1.2), foldhash 0.2.0    |
| Sync Primitives    | parking_lot 0.12.5                            |
| Connection Pooling | deadpool 0.13.0                               |
| Logging            | tracing 0.1.44                                |

## Crate Architecture (27 crates)

```
yazi-fm (binary)                 <- entry point, event loop, rendering
 |
 +-- yazi-actor                  <- actor model, action dispatch, Ctx<'a> wrapper
 +-- yazi-core                   <- business logic: tabs, folders, files, manager
 |    +-- highlighter            <- syntax highlighting integration
 |    +-- picker, input, confirm <- interactive UI components
 |    +-- help, notify           <- informational overlays
 |
 +-- yazi-scheduler              <- async task pool with priority queues
 |    +-- workers                <- file ops: copy, cut, delete, paste, link, trash
 |    +-- hooks                  <- pre/post operation handlers
 |
 +-- yazi-plugin                 <- Lua runtime, plugin loading
 |    +-- elements               <- fetcher, spotter, preloader, previewer
 |    +-- vfs integration        <- Lua-accessible virtual file system
 |    +-- process integration    <- Lua-accessible child processes
 |
 +-- yazi-dds                    <- data distribution service (pub-sub)
 |    +-- client/pump            <- cross-instance communication
 |    +-- state persistence      <- session state sharing
 |
 +-- yazi-config                 <- TOML config parsing
 |    +-- keymap, opener, theme  <- user configuration domains
 |    +-- layout, icons          <- UI configuration
 |    +-- flavors                <- theme variant system
 |
 +-- yazi-runner                 <- Lua script execution
 |    +-- slim runtime           <- core Lua features only
 |    +-- standard runtime       <- full Lua feature set
 |
 +-- yazi-vfs                    <- virtual file system layer
 |    +-- local provider         <- native filesystem
 |    +-- sftp provider          <- remote file access
 |    +-- search engines         <- pluggable search
 |
 +-- yazi-adapter                <- image protocol drivers
 |    +-- kitty (KGP, old KGP)   <- Kitty Graphics Protocol
 |    +-- iip (iTerm2, WezTerm)  <- Inline Images Protocol
 |    +-- sixel                  <- Sixel format (foot, Windows Terminal)
 |    +-- chafa                  <- ASCII art fallback
 |    +-- ueberzug               <- Uberzug++ integration
 |    +-- emulator detection     <- auto-select best protocol
 |
 +-- yazi-widgets                <- custom ratatui widgets
 +-- yazi-binding                <- Lua <-> Rust bindings, cached field access
 +-- yazi-shared                 <- event channel, URL types, ID generation
 +-- yazi-term                   <- terminal rendering, synchronized updates
 +-- yazi-fs                     <- file/path abstractions, sorting, filtering
 +-- yazi-tty                    <- low-level TTY I/O
 +-- yazi-watcher                <- filesystem change monitoring (notify)
 +-- yazi-emulator               <- terminal detection & capabilities
 +-- yazi-cli (ya binary)        <- companion CLI tool
 +-- yazi-proxy                  <- inter-process messaging
 +-- yazi-parser                 <- action specification parser
 +-- yazi-boot                   <- bootstrap & argument parsing
 +-- yazi-sftp                   <- SFTP provider implementation
 +-- yazi-macro                  <- mod_pub!, mod_flat!, act! macros
 +-- yazi-codegen                <- compile-time code generation
 +-- yazi-build                  <- build utilities
 +-- yazi-ffi / yazi-shim        <- FFI layer & compatibility shims
```

## Design Patterns

### 1. Event-Driven Architecture

All interaction flows through an unbounded async mpsc channel. Event types:

| Event     | Purpose                          |
| --------- | -------------------------------- |
| `Call`    | Single action dispatch           |
| `Seq`    | Batch of actions (ordered)       |
| `Render` | Request UI redraw                |
| `Key`    | Keyboard input                   |
| `Mouse`  | Mouse input                      |
| `Resize` | Terminal resize                  |
| `Focus`  | Window focus change              |
| `Paste`  | Bracketed paste content          |

A central dispatcher in `yazi-fm` routes each event to the appropriate handler. Actions use layered naming (`app:quit`, `mgr:open`, `tab:cd`) for clear routing.

### 2. Actor Model

Each action executes within an `Actor` implementing two methods:

- **`hook`** — preflight validation/transformation before execution
- **`act`** — the actual operation

The `act!` macro provides compile-time routing from `layer:name` strings to concrete actor implementations. A `Ctx<'a>` wrapper carries the execution context.

### 3. Priority Task Scheduling

File operations are scheduled through `async-priority-channel` with three tiers:

| Priority | Use Case                              |
| -------- | ------------------------------------- |
| HIGH     | User-initiated actions, cancellation  |
| NORMAL   | Standard file operations              |
| LOW      | Background preloading, thumbnailing   |

Workers execute tasks from a pool. `CompletionToken` enables cooperative cancellation. Hook system supports pre/post operation logic (transaction-like semantics).

### 4. Rendering

Two rendering modes minimize terminal output:

- **Full render** — redraws entire frame (on structural changes)
- **Partial render** — redraws only changed regions (on data updates)

Key mechanisms:
- `NEED_RENDER` atomic flag coalesces rapid render requests
- 10ms debounce between frames
- `BeginSynchronizedUpdate` prevents visual tearing
- Immediate-mode rendering via ratatui (no retained widget tree)

### 5. Lua Plugin System

Two Lua runtime tiers:

| Runtime      | Scope                            |
| ------------ | -------------------------------- |
| **Slim**     | Core features, config access     |
| **Standard** | Full API: filesystem, processes  |

Plugin types, resolved by MIME type:

| Type          | Purpose                                         |
| ------------- | ------------------------------------------------ |
| `fetcher`    | Retrieve metadata for files                      |
| `preloader`  | Background-load data for fast access             |
| `spotter`    | Identify file types beyond extension matching     |
| `previewer`  | Render file previews in the preview pane          |

Lua code can subscribe to DDS events, define custom commands, and extend the UI without recompilation.

### 6. Data Distribution Service (DDS)

Built-in pub-sub for cross-instance communication:

- No external server process required
- Lua-based message format
- Each yazi session gets a unique `YAZI_ID`
- Supports state persistence and broadcasting
- Enables multi-window workflows (e.g., dual-pane across terminals)

### 7. Image Preview Adapter Chain

Auto-detects terminal capabilities and selects the best protocol:

```
Kitty Graphics Protocol (KGP)
  -> Inline Images Protocol (iTerm2, WezTerm, VSCode, Warp)
    -> Sixel (foot, Windows Terminal, Black Box)
      -> Chafa (ASCII art fallback)
        -> Uberzug++ (X11/Wayland external renderer)
```

Emulator detection runs at startup via `yazi-emulator`. ICC color profile handling ensures correct color rendering.

### 8. Virtual File System (VFS)

Provider-based abstraction over different backends:

- **Local** — native filesystem via `yazi-fs`
- **SFTP** — remote access via `russh` with connection pooling (`deadpool`)
- Pluggable search engines per provider
- `typed-path` for cross-platform path handling

## Performance

| Technique                | Purpose                                |
| ------------------------ | -------------------------------------- |
| jemalloc                 | Reduced fragmentation on Linux         |
| xxhash3 / foldhash       | Fast non-cryptographic hashing         |
| LRU cache                | Hot-path data reuse                    |
| Priority channels        | Responsive UX under load               |
| Partial rendering        | Only redraw changed terminal regions   |
| Connection pooling       | Reuse SFTP connections                 |
| Render debounce (10ms)   | Coalesce rapid sequential updates      |
| Synchronized output      | Eliminate visual tearing               |
| parking_lot mutexes      | Faster locking than std::sync          |

## Configuration

All config is TOML-based, loaded at startup by `yazi-config`:

| File           | Purpose                              |
| -------------- | ------------------------------------ |
| `yazi.toml`   | Core settings (manager, preview)     |
| `keymap.toml` | Key bindings per layer               |
| `theme.toml`  | Colors, borders, icons               |

Flavors (theme variants) allow bundling theme + icon overrides as distributable packages. A preset system provides defaults that users override selectively.

## Key Architectural Decisions

1. **27 focused crates** — strict separation of concerns; each crate has a single responsibility
2. **Fully async, non-blocking** — UI thread never blocks on I/O, even for remote SFTP
3. **Lua over WASM/native plugins** — lower barrier to entry, no recompilation needed
4. **Built-in DDS** — cross-instance coordination without external dependencies
5. **Adapter pattern for image protocols** — single interface, broad terminal compatibility
6. **Actor model for actions** — composable, testable command handlers with preflight hooks
7. **Dual Lua runtimes** — slim runtime keeps core fast; standard runtime available when needed
8. **Priority scheduling** — user actions always preempt background work
