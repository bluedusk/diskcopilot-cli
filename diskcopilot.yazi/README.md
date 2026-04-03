# diskcopilot.yazi

Yazi plugin for disk scanning and analytics powered by [diskcopilot-cli](https://github.com/bluedusk/diskcopilot-cli).

## Requirements

- [Yazi](https://yazi-rs.github.io/) (v0.4+)
- `diskcopilot` CLI in your `$PATH`

## Install

```bash
# Option 1: Yazi package manager
ya pkg add diskcopilot/diskcopilot.yazi

# Option 2: Manual
git clone <repo> ~/.config/yazi/plugins/diskcopilot.yazi
```

## Keybindings

Add to `~/.config/yazi/keymap.toml`:

```toml
[[mgr.prepend_keymap]]
on   = "S"
run  = "plugin diskcopilot --args='scan'"
desc = "Scan current directory"

[[mgr.prepend_keymap]]
on   = ["d", "l"]
run  = "plugin diskcopilot --args='large-files'"
desc = "Show large files"

[[mgr.prepend_keymap]]
on   = ["d", "u"]
run  = "plugin diskcopilot --args='duplicates'"
desc = "Find duplicate files"

[[mgr.prepend_keymap]]
on   = ["d", "a"]
run  = "plugin diskcopilot --args='dev-artifacts'"
desc = "Show dev artifact directories"

[[mgr.prepend_keymap]]
on   = ["d", "r"]
run  = "plugin diskcopilot --args='recent'"
desc = "Show recently modified files"

[[mgr.prepend_keymap]]
on   = ["d", "o"]
run  = "plugin diskcopilot --args='old'"
desc = "Show old files"

[[mgr.prepend_keymap]]
on   = ["d", "t"]
run  = "plugin diskcopilot --args='tree'"
desc = "Show directory size tree"

[[mgr.prepend_keymap]]
on   = ["d", "i"]
run  = "plugin diskcopilot --args='info'"
desc = "Show scan info"
```

## Directory Previewer (optional)

Show disk analytics when hovering directories. Add to `~/.config/yazi/yazi.toml`:

```toml
[[plugin.prepend_previewers]]
name = "*/"
run  = "diskcopilot"
```

## Usage

1. Navigate to a directory in Yazi
2. Press `S` to scan it with diskcopilot
3. Use `d` + key to view analytics:
   - `dl` — large files
   - `du` — duplicates
   - `da` — dev artifacts (node_modules, target, etc.)
   - `dr` — recent files
   - `do` — old files
   - `dt` — directory size tree
   - `di` — scan info

With the previewer enabled, hovering any scanned directory shows a size breakdown in the preview pane.
