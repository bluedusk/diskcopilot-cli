// ---------------------------------------------------------------------------
// Icon helpers (Nerd Font required in the terminal font)
// ---------------------------------------------------------------------------

/// Return the icon for a file or directory entry.
///
/// - Directories get a folder icon (open or closed).
/// - Files get an icon based on their file extension.
/// - Falls back to a generic file icon.
pub fn icon_for(name: &str, is_dir: bool, is_open: bool) -> &'static str {
    if is_dir {
        if is_open {
            "󰝰 " // nf-md-folder_open
        } else {
            "󰉋 " // nf-md-folder
        }
    } else {
        // Try well-known filenames first
        if let Some(icon) = icon_for_name(name) {
            return icon;
        }

        // Fall back to extension-based lookup
        let ext = name
            .rsplit('.')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();

        match ext.as_str() {
            "rs" => " ",          // nf-dev-rust
            "py" => " ",          // nf-dev-python
            "js" | "mjs" | "cjs" => " ",  // nf-dev-javascript
            "ts" | "mts" | "cts" => " ",  // nf-dev-typescript
            "html" | "htm" => " ",        // nf-dev-html5
            "css" | "scss" | "sass" => " ", // nf-dev-css3
            "json" => " ",         // nf-seti-json
            "yaml" | "yml" => " ", // nf-seti-yaml
            "toml" => " ",         // nf-seti-toml (reuse rust icon)
            "md" | "mdx" => " ",   // nf-dev-markdown
            "txt" => "󰈙 ",         // nf-md-file_document
            "pdf" => " ",          // nf-seti-pdf
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "ico" => " ", // nf-dev-image
            "mp4" | "mkv" | "avi" | "mov" | "webm" => " ", // nf-fa-film
            "mp3" | "flac" | "wav" | "ogg" | "aac" | "m4a" => " ", // nf-fa-music
            "zip" | "tar" | "gz" | "bz2" | "xz" | "zst" | "7z" | "rar" => " ", // nf-fa-file_archive
            "log" => "󰌱 ",          // nf-md-math_log (log file)
            "sh" | "bash" | "zsh" | "fish" => " ", // nf-dev-terminal
            "c" | "h" => " ",      // nf-custom-c
            "cpp" | "cxx" | "cc" | "hpp" => " ", // nf-custom-cpp
            "go" => " ",           // nf-dev-go
            "java" | "class" | "jar" => " ", // nf-dev-java
            "rb" => " ",           // nf-dev-ruby
            "php" => " ",          // nf-dev-php
            "swift" => " ",        // nf-dev-swift
            "kt" | "kts" => " ",   // nf-custom-kotlin
            "sql" | "db" | "sqlite" => " ", // nf-fa-database
            "lock" => " ",         // nf-fa-lock
            "env" => " ",          // nf-fa-key
            "exe" | "dll" | "so" | "dylib" => " ", // nf-fa-cog
            _ => " ",              // nf-fa-file (default)
        }
    }
}

/// Return an icon for a well-known filename (case-insensitive).
pub fn icon_for_name(name: &str) -> Option<&'static str> {
    match name {
        "Makefile" | "makefile" | "GNUmakefile" => Some(" "),   // nf-seti-makefile
        "Dockerfile" | "dockerfile" => Some(" "),               // nf-dev-docker
        "LICENSE" | "LICENCE" | "LICENSE.txt" | "LICENCE.txt" => Some(" "), // nf-fa-balance_scale
        "README" | "README.md" | "README.txt" | "README.rst" => Some(" "),  // nf-fa-book
        ".gitignore" | ".gitattributes" | ".gitmodules" => Some(" "),       // nf-dev-git
        "Cargo.toml" | "Cargo.lock" => Some(" "),               // nf-dev-rust
        "package.json" | "package-lock.json" => Some(" "),      // nf-dev-npm
        "yarn.lock" => Some(" "),                               // nf-dev-yarn
        "pnpm-lock.yaml" => Some(" "),                          // nf-seti-npm (reuse)
        ".env" | ".env.local" | ".env.example" => Some(" "),    // nf-fa-key
        "docker-compose.yml" | "docker-compose.yaml" => Some(" "), // nf-dev-docker
        ".DS_Store" => Some(" "),                               // nf-fa-apple
        _ => None,
    }
}
