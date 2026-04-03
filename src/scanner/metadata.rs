/// Return the lowercase file extension from a filename, or `None`.
///
/// Examples: `"photo.JPG"` → `Some("jpg")`, `"Makefile"` → `None`.
pub fn file_extension(name: &str) -> Option<String> {
    let name = name.trim_end_matches('/');
    let dot_pos = name.rfind('.')?;
    // Dot at position 0 means hidden file with no extension (.bashrc)
    // Dot as last char means no extension (file.)
    if dot_pos == 0 || dot_pos == name.len() - 1 {
        return None;
    }
    Some(name[dot_pos + 1..].to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_extension() {
        assert_eq!(file_extension("photo.JPG"), Some("jpg".into()));
        assert_eq!(file_extension("archive.tar.gz"), Some("gz".into()));
        assert_eq!(file_extension("Makefile"), None);
        assert_eq!(file_extension(".bashrc"), None);
        assert_eq!(file_extension("file."), None);
        assert_eq!(file_extension("doc.PDF"), Some("pdf".into()));
        assert_eq!(file_extension("no_ext"), None);
    }
}
