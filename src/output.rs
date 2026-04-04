use crate::cache::reader::{CleanupSummary, DuplicateGroup, FileRow, ScanMeta, TreeNode};
use crate::format::format_size;

pub fn print_file_rows(rows: &[FileRow]) {
    if rows.is_empty() {
        println!("  No results.");
        return;
    }
    println!("{:>10}  Path", "Size");
    println!("{:>10}  ----", "----");
    for row in rows {
        println!("{:>10}  {}", format_size(row.disk_size), row.full_path);
    }
    println!("\n  {} items", rows.len());
}

pub fn print_tree_nodes(nodes: &[TreeNode]) {
    if nodes.is_empty() {
        println!("  No results.");
        return;
    }
    println!("{:>10}  {:>6}  Name", "Size", "Files");
    println!("{:>10}  {:>6}  ----", "----", "-----");
    for node in nodes {
        println!(
            "{:>10}  {:>6}  {}",
            format_size(node.disk_size),
            node.file_count,
            node.name
        );
    }
    println!("\n  {} items", nodes.len());
}

pub fn print_tree(node: &TreeNode, _indent: usize) {
    let root_size = node.disk_size;
    println!("\x1b[1m{}\x1b[0m  {}", node.name, format_size(root_size));
    print_children(&node.children, root_size, "");
}

fn print_children(children: &[TreeNode], root_size: u64, prefix: &str) {
    for (i, child) in children.iter().enumerate() {
        let is_last = i == children.len() - 1;
        let connector = if is_last { "└── " } else { "├── " };
        let extension = if is_last { "    " } else { "│   " };

        let size_str = format_size(child.disk_size);
        let pct = if root_size > 0 {
            (child.disk_size as f64 / root_size as f64 * 100.0) as u64
        } else {
            0
        };

        let color = if pct > 50 { "\x1b[31m" } else if pct > 20 { "\x1b[33m" } else if pct > 5 { "\x1b[0m" } else { "\x1b[90m" };
        let reset = "\x1b[0m";

        let bar_width = 10;
        let filled = ((pct as usize) * bar_width / 100).min(bar_width);
        let bar: String = "█".repeat(filled) + &"░".repeat(bar_width - filled);

        println!(
            "{}{}{}{:>9}{} {:>3}% {} {}",
            prefix, connector, color, size_str, reset, pct, bar, child.name
        );

        if !child.children.is_empty() {
            let child_prefix = format!("{}{}", prefix, extension);
            print_children(&child.children, root_size, &child_prefix);
        }
    }
}

pub fn print_duplicate_groups(groups: &[DuplicateGroup]) {
    if groups.is_empty() {
        println!("  No duplicates found.");
        return;
    }
    println!("Found {} duplicate groups:\n", groups.len());
    for (i, group) in groups.iter().enumerate() {
        println!(
            "  Group {} — {} × {} files",
            i + 1,
            format_size(group.size),
            group.count
        );
    }
}

pub fn print_summary(summary: &CleanupSummary) {
    let bold = "\x1b[1m";
    let green = "\x1b[32m";
    let yellow = "\x1b[33m";
    let reset = "\x1b[0m";

    println!("\n{bold}Disk Usage Summary{reset}");
    println!("{}",  "─".repeat(18));
    println!(
        "  Total scanned:  {green}{}{reset} ({} files)",
        crate::format::format_size(summary.total_size),
        summary.total_files
    );

    println!("\n{bold}Top Large Files{reset}");
    if summary.large_files.is_empty() {
        println!("  No large files found.");
    } else {
        for row in &summary.large_files {
            println!("  {green}{:>10}{reset}  {}", crate::format::format_size(row.disk_size), row.full_path);
        }
    }

    println!("\n{bold}Dev Artifacts (cleanable){reset}");
    if summary.dev_artifacts.is_empty() {
        println!("  None found.");
    } else {
        // Group by name to count instances
        let mut by_name: std::collections::HashMap<&str, (u64, usize)> = std::collections::HashMap::new();
        for node in &summary.dev_artifacts {
            let entry = by_name.entry(node.name.as_str()).or_insert((0, 0));
            entry.0 += node.disk_size;
            entry.1 += 1;
        }
        let mut grouped: Vec<_> = by_name.into_iter().collect();
        grouped.sort_by(|a, b| b.1.0.cmp(&a.1.0));
        for (name, (size, count)) in &grouped {
            println!(
                "  {yellow}{:>10}{reset}  {} ({} instance{})",
                crate::format::format_size(*size),
                name,
                count,
                if *count == 1 { "" } else { "s" }
            );
        }
        let total_artifacts: u64 = summary.dev_artifacts.iter().map(|n| n.disk_size).sum();
        println!("  Total: {yellow}{}{reset}", crate::format::format_size(total_artifacts));
    }

    println!("\n{bold}Old Files (>1 year){reset}");
    println!(
        "  {yellow}{}{reset} across {} files",
        crate::format::format_size(summary.old_files_size),
        summary.old_files_count
    );

    println!(
        "\n{bold}Potential Savings: {green}{}{reset}",
        crate::format::format_size(summary.potential_savings)
    );
    println!();
}

/// Print a rich post-scan report with tree overview and cleanup highlights.
pub fn print_scan_report(conn: &rusqlite::Connection) {
    let bold = "\x1b[1m";
    let reset = "\x1b[0m";
    let green = "\x1b[32m";
    let yellow = "\x1b[33m";
    let cyan = "\x1b[36m";
    let dim = "\x1b[90m";

    // --- Directory breakdown (top-level, depth 1, top 10 + "other") ---
    if let Ok(root) = crate::cache::reader::load_root(conn) {
        if let Ok(tree) = crate::cache::reader::load_tree_to_depth(conn, root.id, 1) {
            println!("\n{bold}Directory Breakdown{reset}");
            println!("{}", "─".repeat(60));

            let total = tree.disk_size.max(1);
            let bar_max = 30;
            let max_items = 10;
            let mut shown_size: u64 = 0;

            for child in tree.children.iter().take(max_items) {
                let pct = child.disk_size as f64 / total as f64 * 100.0;
                let filled = ((pct as usize) * bar_max / 100).min(bar_max);
                let bar_color = if pct > 50.0 {
                    "\x1b[31m"
                } else if pct > 20.0 {
                    yellow
                } else if pct > 5.0 {
                    green
                } else {
                    dim
                };
                let bar: String = "█".repeat(filled) + &"░".repeat(bar_max - filled);
                let icon = if child.is_dir { "" } else { "" };

                println!(
                    "  {}{:>9}{} {:>5.1}%  {}{}{} {} {}",
                    bold,
                    format_size(child.disk_size),
                    reset,
                    pct,
                    bar_color,
                    bar,
                    reset,
                    icon,
                    child.name
                );
                shown_size += child.disk_size;
            }

            if tree.children.len() > max_items {
                let other_size = total.saturating_sub(shown_size);
                let other_count = tree.children.len() - max_items;
                let pct = other_size as f64 / total as f64 * 100.0;
                println!(
                    "  {dim}{:>9} {:>5.1}%  ... and {} more items{reset}",
                    format_size(other_size),
                    pct,
                    other_count
                );
            }
        }
    }

    // --- Dev artifacts ---
    if let Ok(artifacts) = crate::cache::reader::query_dev_artifacts(conn) {
        if !artifacts.is_empty() {
            let total: u64 = artifacts.iter().map(|a| a.disk_size).sum();
            // Group by name
            let mut by_name: std::collections::HashMap<&str, (u64, usize)> =
                std::collections::HashMap::new();
            for a in &artifacts {
                let e = by_name.entry(a.name.as_str()).or_insert((0, 0));
                e.0 += a.disk_size;
                e.1 += 1;
            }
            let mut grouped: Vec<_> = by_name.into_iter().collect();
            grouped.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));

            println!(
                "\n{bold}Dev Artifacts{reset} {dim}(safe to delete, regenerated by build commands){reset}"
            );
            println!("{}", "─".repeat(60));
            for (name, (size, count)) in grouped.iter().take(5) {
                println!(
                    "  {yellow}{:>9}{reset}  {} {dim}({} instance{}){reset}",
                    format_size(*size),
                    name,
                    count,
                    if *count == 1 { "" } else { "s" }
                );
            }
            println!("  {bold}Total: {yellow}{}{reset}", format_size(total));
        }
    }

    // --- Top large files ---
    if let Ok(large) = crate::cache::reader::query_large_files(conn, 100_000_000, 5) {
        if !large.is_empty() {
            println!("\n{bold}Largest Files{reset}");
            println!("{}", "─".repeat(60));
            for f in &large {
                let ext = f.extension.as_deref().unwrap_or("");
                let ext_display = if ext.is_empty() {
                    String::new()
                } else {
                    format!(" {dim}[{}]{reset}", ext)
                };
                println!(
                    "  {cyan}{:>9}{reset}  {}{}",
                    format_size(f.disk_size),
                    f.full_path,
                    ext_display
                );
            }
        }
    }

    // --- Potential savings ---
    if let Ok(summary) = crate::cache::reader::query_summary(conn) {
        if summary.potential_savings > 0 {
            println!(
                "\n  {bold}Potential savings: {green}{}{reset}",
                format_size(summary.potential_savings)
            );
        }
    }

    println!(
        "\n  {dim}Run 'diskcopilot-cli serve {}' for interactive cleanup{reset}\n",
        crate::cache::reader::load_scan_meta(conn)
            .ok()
            .flatten()
            .map(|m| m.root_path)
            .unwrap_or_else(|| "<path>".into())
    );
}

pub fn print_scan_meta(meta: &ScanMeta) {
    println!("  Root:     {}", meta.root_path);
    println!("  Files:    {}", meta.total_files);
    println!("  Dirs:     {}", meta.total_dirs);
    println!("  Size:     {}", format_size(meta.total_size as u64));
    println!("  Duration: {}ms", meta.scan_duration_ms);
    let dt = meta.scanned_at;
    println!("  Scanned:  {} (unix timestamp)", dt);
}
