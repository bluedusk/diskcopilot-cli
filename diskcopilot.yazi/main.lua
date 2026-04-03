-- diskcopilot.yazi — Yazi plugin for disk scanning and analytics
-- Requires: diskcopilot CLI in $PATH

local json = require("json")

local M = {}

-- ---------------------------------------------------------------------------
-- Helpers
-- ---------------------------------------------------------------------------

-- Get current working directory (sync context)
local get_cwd = ya.sync(function()
	return tostring(cx.active.current.cwd)
end)

-- Get hovered item path (sync context)
local get_hovered = ya.sync(function()
	local h = cx.active.current.hovered
	if h then
		return tostring(h.url), h.cha.is_dir
	end
	return nil, false
end)

-- Run diskcopilot and return parsed JSON output, or nil + error string
local function run_diskcopilot(args)
	local cmd = Command("diskcopilot-cli")
	for _, arg in ipairs(args) do
		cmd = cmd:arg(arg)
	end
	local output, err = cmd:output()
	if not output then
		return nil, "Failed to run diskcopilot: " .. tostring(err)
	end
	if not output.status.success then
		local msg = output.stderr:gsub("%s+$", "")
		return nil, msg ~= "" and msg or "diskcopilot exited with error"
	end
	local stdout = output.stdout:gsub("%s+$", "")
	if stdout == "" or stdout == "null" then
		return nil, nil
	end
	local ok, data = pcall(json.decode, stdout)
	if not ok then
		return nil, "Failed to parse JSON: " .. tostring(data)
	end
	return data, nil
end

-- Format bytes to human-readable (decimal units, matching CLI output)
local function format_size(bytes)
	if not bytes or bytes == 0 then return "0 B" end
	local units = { "B", "KB", "MB", "GB", "TB" }
	local i = 1
	local size = bytes
	while size >= 1000 and i < #units do
		size = size / 1000
		i = i + 1
	end
	if i == 1 then
		return string.format("%d B", size)
	end
	return string.format("%.1f %s", size, units[i])
end

-- ---------------------------------------------------------------------------
-- Entry point — dispatches based on first arg
-- ---------------------------------------------------------------------------

function M:entry(job)
	local action = job.args[1]
	if not action then
		ya.notify({
			title = "diskcopilot",
			content = "Usage: plugin diskcopilot --args='<action>'\nActions: scan, large-files, recent, old, dev-artifacts, duplicates, tree, info",
			level = "warn",
		})
		return
	end

	if action == "scan" then
		self:do_scan()
	elseif action == "large-files" then
		self:do_query("large-files", "Large Files")
	elseif action == "recent" then
		self:do_query("recent", "Recent Files")
	elseif action == "old" then
		self:do_query("old", "Old Files")
	elseif action == "dev-artifacts" then
		self:do_query("dev-artifacts", "Dev Artifacts")
	elseif action == "duplicates" then
		self:do_query("duplicates", "Duplicates")
	elseif action == "tree" then
		self:do_tree()
	elseif action == "info" then
		self:do_info()
	else
		ya.notify({
			title = "diskcopilot",
			content = "Unknown action: " .. action,
			level = "error",
		})
	end
end

-- ---------------------------------------------------------------------------
-- Scan
-- ---------------------------------------------------------------------------

function M:do_scan()
	local cwd = get_cwd()
	ya.notify({
		title = "diskcopilot",
		content = "Scanning " .. cwd .. "...",
		level = "info",
	})

	local cmd = Command("diskcopilot-cli"):arg("scan"):arg(cwd):arg("--full")
	local output, err = cmd:output()

	if not output then
		ya.notify({
			title = "diskcopilot",
			content = "Scan failed: " .. tostring(err),
			level = "error",
		})
		return
	end

	if output.status.success then
		local msg = output.stdout:gsub("%s+$", "")
		ya.notify({
			title = "diskcopilot",
			content = msg ~= "" and msg or "Scan complete",
			level = "info",
		})
	else
		local msg = output.stderr:gsub("%s+$", "")
		ya.notify({
			title = "diskcopilot",
			content = "Scan failed: " .. (msg ~= "" and msg or "unknown error"),
			level = "error",
		})
	end
end

-- ---------------------------------------------------------------------------
-- Query commands (large-files, recent, old, dev-artifacts, duplicates)
-- ---------------------------------------------------------------------------

function M:do_query(subcmd, title)
	local cwd = get_cwd()
	local data, err = run_diskcopilot({ "query", subcmd, cwd, "--json" })

	if err then
		ya.notify({
			title = "diskcopilot",
			content = err,
			level = "error",
		})
		return
	end

	if not data or #data == 0 then
		ya.notify({
			title = title,
			content = "No results found.",
			level = "info",
		})
		return
	end

	-- Build summary
	local lines = {}
	local max_items = 20
	for i, item in ipairs(data) do
		if i > max_items then
			table.insert(lines, string.format("  ... and %d more", #data - max_items))
			break
		end
		local name = item.name or item.hash or "?"
		local size = format_size(item.disk_size or item.size or 0)
		local path = item.full_path or ""
		if path ~= "" then
			table.insert(lines, string.format("  %s  %s", size, path))
		else
			local count = item.file_count or item.count or ""
			if count ~= "" then
				table.insert(lines, string.format("  %s  %s (%s files)", size, name, count))
			else
				table.insert(lines, string.format("  %s  %s", size, name))
			end
		end
	end

	ya.notify({
		title = string.format("%s (%d)", title, #data),
		content = table.concat(lines, "\n"),
		level = "info",
	})
end

-- ---------------------------------------------------------------------------
-- Tree overview
-- ---------------------------------------------------------------------------

function M:do_tree()
	local cwd = get_cwd()
	local data, err = run_diskcopilot({ "query", "tree", cwd, "--depth", "2", "--json" })

	if err then
		ya.notify({
			title = "diskcopilot",
			content = err,
			level = "error",
		})
		return
	end

	if not data then
		ya.notify({
			title = "Tree",
			content = "No data.",
			level = "info",
		})
		return
	end

	-- Flatten tree into lines
	local lines = {}
	local function walk(node, depth)
		local indent = string.rep("  ", depth)
		local icon = node.is_dir and "" or ""
		local size = format_size(node.disk_size or 0)
		table.insert(lines, string.format("%s%s %s (%s)", indent, icon, node.name or "?", size))
		if node.children then
			for _, child in ipairs(node.children) do
				if #lines < 30 then
					walk(child, depth + 1)
				end
			end
		end
	end
	walk(data, 0)

	ya.notify({
		title = "Directory Tree",
		content = table.concat(lines, "\n"),
		level = "info",
	})
end

-- ---------------------------------------------------------------------------
-- Scan info
-- ---------------------------------------------------------------------------

function M:do_info()
	local cwd = get_cwd()
	local data, err = run_diskcopilot({ "query", "info", cwd, "--json" })

	if err then
		ya.notify({
			title = "diskcopilot",
			content = err,
			level = "error",
		})
		return
	end

	if not data then
		ya.notify({
			title = "Scan Info",
			content = "Not scanned. Press S to scan.",
			level = "info",
		})
		return
	end

	local content = string.format(
		"Root:     %s\nFiles:    %s\nDirs:     %s\nSize:     %s\nDuration: %dms",
		data.root_path or "?",
		data.total_files or "?",
		data.total_dirs or "?",
		format_size(data.total_size or 0),
		data.scan_duration_ms or 0
	)

	ya.notify({
		title = "Scan Info",
		content = content,
		level = "info",
	})
end

-- ---------------------------------------------------------------------------
-- Previewer — show directory analytics in preview pane
-- ---------------------------------------------------------------------------

-- Cache: keyed by path string → { lines = {...}, time = os.time() }
local preview_cache = {}

function M:peek(job)
	local path = tostring(job.file.url)

	-- Check cache (valid for 60s)
	local cached = preview_cache[path]
	if cached and (os.time() - cached.time) < 60 then
		ya.preview_widgets(job, { ui.Text(cached.lines):area(job.area) })
		return
	end

	-- Try to get tree data
	local data, err = run_diskcopilot({ "query", "tree", path, "--depth", "1", "--json" })

	if err or not data then
		local msg = err or "Not scanned. Press S to scan."
		preview_cache[path] = {
			lines = { ui.Line(ui.Span(msg):fg("gray")) },
			time = os.time(),
		}
		ya.preview_widgets(job, { ui.Text(preview_cache[path].lines):area(job.area) })
		return
	end

	-- Build preview lines
	local lines = {}
	table.insert(lines, ui.Line({
		ui.Span(data.name or "?"):bold(),
		ui.Span(" (" .. format_size(data.disk_size or 0) .. ")"):fg("gray"),
	}))
	table.insert(lines, ui.Line(ui.Span("")))

	if data.children then
		-- Sort by disk_size descending (should already be sorted from CLI)
		for i, child in ipairs(data.children) do
			if i > job.area.h - 3 then
				table.insert(lines, ui.Line(ui.Span(
					string.format("  ... and %d more", #data.children - i + 1)
				):fg("gray")))
				break
			end
			local icon = child.is_dir and " " or " "
			local size = format_size(child.disk_size or 0)
			local ratio = (data.disk_size and data.disk_size > 0)
				and (child.disk_size or 0) / data.disk_size
				or 0
			local color = ratio > 0.5 and "red" or ratio > 0.2 and "yellow" or "green"

			table.insert(lines, ui.Line({
				ui.Span(string.format("  %9s", size)):fg(color),
				ui.Span("  " .. icon .. (child.name or "?")),
			}))
		end
	end

	-- Cache and display
	preview_cache[path] = { lines = lines, time = os.time() }
	ya.preview_widgets(job, { ui.Text(lines):area(job.area) })
end

function M:seek(job)
	-- Basic scroll support
	ya.mgr_emit("peek", {
		tostring(math.max(0, cx.active.preview.skip + job.units)),
		only_if = job.file.url,
	})
end

return M
