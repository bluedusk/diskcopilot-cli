/// Format bytes into human-readable string (e.g., 1.2 GB, 340 MB, 4.5 KB).
pub fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;

    if bytes >= TB {
        format!("{:.1} TB", bytes as f64 / TB as f64)
    } else if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

/// Parse a size string like "100M", "1G", "500K", "100MB", "1GB" into bytes.
pub fn parse_size(s: &str) -> anyhow::Result<u64> {
    let s = s.trim();
    // Split at the boundary between digits (or '.') and trailing alphabetic characters.
    let split_pos = s
        .rfind(|c: char| !c.is_alphabetic())
        .map(|p| p + 1)
        .unwrap_or(0);
    let (num, unit) = (&s[..split_pos], &s[split_pos..]);

    let value: f64 = num.parse().map_err(|_| anyhow::anyhow!("invalid size: {}", s))?;

    let multiplier: u64 = match unit.to_uppercase().as_str() {
        "K" | "KB" => 1024,
        "M" | "MB" => 1024 * 1024,
        "G" | "GB" => 1024 * 1024 * 1024,
        "T" | "TB" => 1024 * 1024 * 1024 * 1024,
        "" | "B" => 1,
        _ => return Err(anyhow::anyhow!("unknown size unit: {}", unit)),
    };

    Ok((value * multiplier as f64) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_size() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(1048576), "1.0 MB");
        assert_eq!(format_size(1073741824), "1.0 GB");
        assert_eq!(format_size(1099511627776), "1.0 TB");
    }

    #[test]
    fn test_parse_size() {
        assert_eq!(parse_size("100M").unwrap(), 104857600);
        assert_eq!(parse_size("1G").unwrap(), 1073741824);
        assert_eq!(parse_size("500K").unwrap(), 512000);
        assert_eq!(parse_size("1024").unwrap(), 1024);
        assert!(parse_size("abc").is_err());
    }

    #[test]
    fn test_parse_size_multi_char_units() {
        assert_eq!(parse_size("100MB").unwrap(), 104857600);
        assert_eq!(parse_size("1GB").unwrap(), 1073741824);
        assert_eq!(parse_size("500KB").unwrap(), 512000);
        assert_eq!(parse_size("2TB").unwrap(), 2 * 1024 * 1024 * 1024 * 1024);
    }
}
