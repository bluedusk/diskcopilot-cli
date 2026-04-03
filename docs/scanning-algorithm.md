# Scanning Algorithm: Evolution & Design Decisions

## Summary

The scanner went through 6 iterations to achieve its current performance: **12s default / 17s full scan** of a home directory with 1.4M files on APFS. The final design uses macOS `getattrlistbulk(2)` for parallel bulk metadata retrieval, with a jwalk fallback for non-APFS volumes.

## Approaches Tried (in order)

### Attempt 1: jwalk collect-all + two-pass (FAILED)

**Approach:** Collect all jwalk entries into a `Vec`, then iterate twice — first pass for dirs, second for files.

```rust
let entries: Vec<_> = WalkDir::new(root).into_iter().filter_map(|e| e.ok()).collect();
// Pass 1: dirs
for entry in &entries { if entry.is_dir() { ... } }
// Pass 2: files
for entry in &entries { if !entry.is_dir() { ... } }
```

**Result:** Stuck/hung on large directories. 185s for home directory scan.

**Why it failed:**
- Collected 1.5M+ entries into memory before processing anything
- No progress feedback during collection phase — appeared stuck
- Each entry called `extract_metadata()` which did a second `symlink_metadata()` call — **double stat per file**
- `HashMap<PathBuf, i64>` for path→dir_id mapping was expensive with 175k keys
- `.sort(true)` on jwalk forced sequential ordering, killing parallelism

**Lesson:** Never collect the entire walk into memory. Stream entries or buffer lightweight records.

### Attempt 2: jwalk streaming single-pass (PARTIAL FIX)

**Approach:** Stream entries from jwalk, process dirs inline, buffer files as lightweight structs.

**Result:** Walk phase fast (~12s), but `finalize()` took 171s.

**Why it was slow:**
- `compute_dir_sizes()` Step 1 used correlated subqueries:
  ```sql
  UPDATE dirs SET file_count = (SELECT COUNT(*) FROM files WHERE files.dir_id = dirs.id) ...
  ```
  This ran 4 subqueries × 175k dirs = 700k queries. **47 seconds** for this single SQL statement.
- Root cause: the UPDATE hit ALL 175k dirs even though only 6,101 had any files (97% were empty).

**Lesson:** Never run correlated subqueries against the full table. Filter first.

### Attempt 3: Sparse UPDATE with WHERE IN (BREAKTHROUGH)

**Approach:** Aggregate file stats into a temp table, then UPDATE only dirs present in that table.

```sql
CREATE TEMP TABLE dir_direct AS SELECT dir_id AS id, COUNT(*) AS fc, ... FROM files GROUP BY dir_id;
CREATE INDEX temp.idx_dd_id ON dir_direct(id);
UPDATE dirs SET ... WHERE dirs.id IN (SELECT id FROM dir_direct);
```

**Result:** Step 1 went from **47s to 0.01s**. Total scan: 60s → 12.5s.

**Why it worked:** Only 6k of 175k dirs needed updating. The indexed temp table made the JOIN instant.

**Lesson:** 97% of directories in a typical macOS home dir are empty (`.git/objects/XX`, `node_modules` nested dirs). Always filter before bulk UPDATE.

### Attempt 4: jwalk process_read_dir for parallel stat (IMPROVEMENT)

**Approach:** Use `WalkDirGeneric` with `process_read_dir` callback to run stat() on jwalk's rayon thread pool instead of sequentially on the main thread.

```rust
let walker = WalkDirGeneric::<((), FileStat)>::new(root)
    .process_read_dir(move |_depth, _path, _state, children| {
        children.retain_mut(|entry| {
            // stat() happens HERE, on parallel rayon threads
            entry.client_state = FileStat { ... };
            true
        });
    });
```

**Result:** Walk stayed ~12s (I/O bound), but CPU user time dropped. Still limited by single stat() per file.

**Why it helped but wasn't enough:** Parallelized stat calls across cores, but each file still required its own `lstat()` syscall — 1.5M syscalls total.

**Lesson:** `process_read_dir` is the right pattern for parallelizing per-entry work in jwalk. The main thread must drain the iterator fast or it blocks rayon's threads.

### Attempt 5: Empty dir pruning (OPTIMIZATION)

**Approach:** Only write dirs to SQLite that are ancestors of at least one file.

```
Before: 175k dir INSERTs → After: 60k dir INSERTs (65% reduction)
```

**Result:** Reduced SQLite write time and cache size (10.5 MB → 1.8 MB for default scan).

**Lesson:** On macOS, most directories are empty leaves (`.git/objects`, `node_modules` nesting). Track needed dirs via ancestor chain marking during file processing.

### Attempt 6: getattrlistbulk — final design (CURRENT)

**Approach:** Port the `getattrlistbulk(2)` syscall from the companion GUI app. This macOS-specific syscall returns metadata for **all entries in a directory** in a single call, vs readdir + stat per file.

**Result:** 
- Default scan: **12s** (was 22s with jwalk)  
- Full scan: **17s** (was 24s)
- CPU user time: **1.2s** (was 13s) — 10x reduction
- Size accuracy: **114.2 GB** vs DaisyDisk's **115.8 GB** (98.6% match)

**Why it's fast:**
- One syscall per directory instead of per-file. ~175k syscalls instead of ~1.5M.
- Parallel directory processing via `rayon::scope` — each directory is processed on a rayon thread.
- Results streamed via `crossbeam_channel` to a collector thread that buffers in memory.
- SQLite writes happen after the walk completes (no I/O contention during traversal).
- Bundle directories (`.app`, `.framework`) sized as opaque entries on parallel threads, not descended into.

## Current Architecture

```
Phase 1: Parallel Walk (rayon threads)
  ├── getattrlistbulk per directory (bulk metadata)
  ├── Filter: symlinks, cross-device, firmlink exclusions, bundles
  ├── Bundle sizing on parallel threads
  └── Results → crossbeam channel → collector

Phase 2: Collect (single thread)
  ├── Receive WalkMsg from channel
  ├── Assign dir IDs, buffer files
  ├── Track needed dirs (ancestor chain marking)
  └── Update progress counters

Phase 3: SQLite Write (single thread)
  ├── Write needed dirs only (prune empty leaves)
  ├── Write file entries
  ├── PRAGMA journal_mode=OFF (no journal needed — cache is rebuildable)
  └── Batch size: 500k entries per flush

Phase 4: Finalize
  ├── compute_dir_sizes(): grouped aggregation + sparse UPDATE
  ├── Create indexes (deferred for bulk insert speed)
  └── Write scan_meta with accurate DB totals
```

## Key Learnings & Cautions

### Performance
1. **SQLite correlated subqueries are lethal at scale.** The `UPDATE dirs SET x = (SELECT ... FROM files WHERE files.dir_id = dirs.id)` pattern that works for 1k rows becomes catastrophic at 175k rows. Always use temp table + JOIN.
2. **97% of dirs are empty.** Never iterate/update all dirs when only ~3% matter.
3. **stat() is the bottleneck, not I/O.** On APFS with SSD, the kernel can serve data fast — but 1.5M individual stat syscalls have overhead regardless. `getattrlistbulk` batches this.
4. **Don't block jwalk's rayon threads.** If the main thread is slow (e.g., SQLite writes), rayon threads back up and parallelism is lost. Drain the iterator fast, buffer in memory, write to DB later.
5. **PathBuf allocations add up.** 175k `HashMap<PathBuf, i64>` entries consume significant memory and CPU for hashing. Minimize PathBuf cloning.

### Accuracy
6. **Binary vs decimal GB.** macOS/Finder/DaisyDisk use decimal (1 GB = 10^9 bytes). Using binary GiB and labeling it "GB" makes numbers look 7% smaller. Always use decimal for user-facing output.
7. **APFS firmlinks cause double-counting.** `/Users`, `/Library`, `/Applications` are firmlinks into `/System/Volumes/Data`. When scanning `/`, exclude `/System/Volumes/Data` or you count everything twice.
8. **Bundle dirs contain thousands of files.** `.app`, `.framework`, `.bundle`, `.plugin`, `.kext` — not descending into them saves millions of stat calls. Size them as opaque entries on parallel threads.
9. **Cross-device mounts.** Always check `dev_id` to avoid scanning network shares, USB drives, or APFS sub-volumes mounted under the scan path.
10. **`getattrlistbulk` returns parent's dev_id for children.** Mount points under `/System/Volumes` and `/Volumes` need explicit stat() to get the real device.

### macOS-Specific
11. **SIP protects `/System`.** Even root can't read the sealed system volume. ~30 GB of data is fundamentally inaccessible without a privileged helper.
12. **Full Disk Access (TCC)** is needed to read `~/Library/Mail`, `~/Library/Messages`, etc. CLI tools must be manually added in System Settings.
13. **`getattrlistbulk` only works on APFS/HFS+.** FAT32/exFAT/NTFS return `allocsize=0`. Always probe with `supports_bulk_attrs()` and fall back to jwalk.
14. **`alloc_size` from getattrlistbulk is physical allocation.** It accounts for APFS compression and sparse files. Use it for both `logical_size` and `disk_size` fields when the real logical size isn't available from this syscall.

### Benchmarks (home dir, ~1.4M files, APFS SSD)

| Metric | Attempt 1 | Attempt 3 | Final (getattrlistbulk) |
|--------|-----------|-----------|------------------------|
| Wall time | 185s | 12.5s | 12-17s |
| CPU user | 170s | 5.5s | 1.2s |
| CPU system | 40s | 36s | 20s |
| Files/sec | 60 | 826 | 85,000 |
| Cache size | 10.5 MB | 1.8 MB | 1.8 MB |
| Size accuracy | 94.5 GB (binary, wrong) | 106.4 GB (binary) | 114.2 GB (decimal, correct) |

### Comparison with other tools

| Tool | Time (home dir) | Method |
|------|----------------|--------|
| `du -sh ~` | 37s | single-threaded readdir+stat |
| diskcopilot jwalk | 22s | parallel readdir+stat |
| diskcopilot bulk | 12-17s | parallel getattrlistbulk |
| DaisyDisk | 8-15s | getattrlistbulk + privileged helper |
| GUI app (x-scanner) | ~15s | getattrlistbulk |
