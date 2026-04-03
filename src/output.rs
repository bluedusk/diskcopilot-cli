use crate::cache::reader::{DuplicateGroup, FileRow, ScanMeta, TreeNode};
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

pub fn print_tree(node: &TreeNode, indent: usize) {
    let prefix = "  ".repeat(indent);
    let icon = if node.is_dir { "📁" } else { "📄" };
    println!(
        "{}{} {} ({})",
        prefix,
        icon,
        node.name,
        format_size(node.disk_size)
    );
    for child in &node.children {
        print_tree(child, indent + 1);
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

pub fn print_scan_meta(meta: &ScanMeta) {
    println!("  Root:     {}", meta.root_path);
    println!("  Files:    {}", meta.total_files);
    println!("  Dirs:     {}", meta.total_dirs);
    println!("  Size:     {}", format_size(meta.total_size as u64));
    println!("  Duration: {}ms", meta.scan_duration_ms);
    let dt = meta.scanned_at;
    println!("  Scanned:  {} (unix timestamp)", dt);
}
