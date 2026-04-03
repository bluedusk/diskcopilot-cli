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

pub fn print_scan_meta(meta: &ScanMeta) {
    println!("  Root:     {}", meta.root_path);
    println!("  Files:    {}", meta.total_files);
    println!("  Dirs:     {}", meta.total_dirs);
    println!("  Size:     {}", format_size(meta.total_size as u64));
    println!("  Duration: {}ms", meta.scan_duration_ms);
    let dt = meta.scanned_at;
    println!("  Scanned:  {} (unix timestamp)", dt);
}
