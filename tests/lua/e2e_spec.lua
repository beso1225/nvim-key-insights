local collector = require("key-insights.collector")
local config = require("key-insights.config")
local keymap_snapshot = require("key-insights.keymap_snapshot")
local process = require("key-insights.process")
local report = require("key-insights.report")
local storage = require("key-insights.storage")

local analyzer = os.getenv("KEY_INSIGHTS_BIN")
assert(type(analyzer) == "string" and analyzer ~= "", "KEY_INSIGHTS_BIN must identify the built analyzer")

local forbidden = {
  "BUFFER_PATH_SECRET",
  "INSERT_TEXT_SECRET",
  "COMMAND_TEXT_SECRET",
  "SEARCH_TEXT_SECRET",
  "MAPPING_EXPANSION_SECRET",
  "GLOBAL_MAPPING_RHS_SECRET",
  "BUFFER_MAPPING_RHS_SECRET",
  "REMOVED_MAPPING_RHS_SECRET",
  "COLLISION_GLOBAL_RHS_SECRET",
  "COLLISION_BUFFER_RHS_SECRET",
}

local function assert_absent(boundary, contents, values)
  for _, value in ipairs(values) do
    assert(not contents:find(value, 1, true), boundary .. " leaked " .. value)
  end
end

local root = vim.fn.tempname()
local session_directory = vim.fs.joinpath(root, "sessions")
local report_directory = vim.fs.joinpath(root, "reports")
local store = storage.new({ directory = session_directory })
local session_ids = { "SESSION_BOUNDARY_ONE", "SESSION_BOUNDARY_TWO" }
local session_index = 0
local callback = nil
local mode = "n"
local command_type = ""
local now_ms = 0
local current_buffer_id = vim.api.nvim_get_current_buf()
local original_buffer_name = vim.api.nvim_buf_get_name(current_buffer_id)
vim.api.nvim_buf_set_name(current_buffer_id, "/private/BUFFER_PATH_SECRET/source.lua")

local mapping_ids = {
  global = assert(keymap_snapshot.mapping_id("normal", "global", { "z", "1" })),
  removed = assert(keymap_snapshot.mapping_id("normal", "global", { "z", "2" })),
  buffer = assert(keymap_snapshot.mapping_id("normal", "buffer", { "z", "3" })),
  collision_global = assert(keymap_snapshot.mapping_id("normal", "global", { "z", "4" })),
  collision_buffer = assert(keymap_snapshot.mapping_id("normal", "buffer", { "z", "4" })),
}

vim.keymap.set("n", "z1", ":echo 'GLOBAL_MAPPING_RHS_SECRET'<CR>")
vim.keymap.set("n", "z2", ":echo 'REMOVED_MAPPING_RHS_SECRET'<CR>")
vim.keymap.set("n", "z4", ":echo 'COLLISION_GLOBAL_RHS_SECRET'<CR>")
vim.keymap.set("n", "z3", ":echo 'BUFFER_MAPPING_RHS_SECRET'<CR>", { buffer = current_buffer_id })
vim.keymap.set("n", "z4", ":echo 'COLLISION_BUFFER_RHS_SECRET'<CR>", { buffer = current_buffer_id })

local instance = collector.new({
  clock_ms = function()
    return now_ms
  end,
  current_buffer = function()
    return {
      id = current_buffer_id,
      buftype = "",
      filetype = "lua",
      name = "/private/BUFFER_PATH_SECRET/source.lua",
    }
  end,
  current_cmdtype = function()
    return command_type
  end,
  current_mode = function()
    return mode
  end,
  new_session_id = function()
    session_index = session_index + 1
    return session_ids[session_index]
  end,
  open_session = function(session_id)
    return store:open_session(session_id)
  end,
  options = config.defaults(),
  register_on_key = function(handler)
    callback = handler
    return function()
      callback = nil
    end
  end,
})

assert(instance:start())
assert(callback("MAPPING_EXPANSION_SECRET", "j") == nil)
now_ms = 2
assert(callback("GLOBAL_MAPPING_RHS_SECRET", "z1") == nil)
now_ms = 4
assert(callback("REMOVED_MAPPING_RHS_SECRET", "z2") == nil)
now_ms = 6
assert(callback("BUFFER_MAPPING_RHS_SECRET", "z3") == nil)
now_ms = 8
assert(callback("COLLISION_BUFFER_RHS_SECRET", "z4") == nil)
now_ms = 10
mode = "i"
assert(callback("MAPPING_EXPANSION_SECRET", "INSERT_TEXT_SECRET") == nil)
now_ms = 20
mode = "c"
command_type = ":"
assert(callback("MAPPING_EXPANSION_SECRET", "COMMAND_TEXT_SECRET") == nil)
now_ms = 30
command_type = "/"
assert(callback("MAPPING_EXPANSION_SECRET", "SEARCH_TEXT_SECRET") == nil)
now_ms = 40
mode = "n"
command_type = ""
assert(callback("MAPPING_EXPANSION_SECRET", "k") == nil)
now_ms = 50
assert(instance:pause())
assert(instance:start())
now_ms = 60
assert(callback("MAPPING_EXPANSION_SECRET", "j") == nil)
now_ms = 70
assert(instance:stop())

vim.keymap.del("n", "z2")

now_ms = 100
assert(instance:start())
assert(callback("MAPPING_EXPANSION_SECRET", "xx") == nil)
now_ms = 130
assert(instance:stop())

local logs = vim.fn.glob(session_directory .. "/*.jsonl", false, true)
table.sort(logs)
assert(#logs == 2, "the collector must finalize one log per session")
local jsonl = {}
for _, log_path in ipairs(logs) do
  local stat = vim.uv.fs_lstat(log_path)
  assert(stat.type == "file" and stat.nlink == 1 and stat.mode % 4096 == 384)
  table.insert(jsonl, table.concat(vim.fn.readfile(log_path, "b"), "\n"))
end
local combined_jsonl = table.concat(jsonl, "\n")
assert_absent("collector JSONL", combined_jsonl, forbidden)
for _, session_id in ipairs(session_ids) do
  assert(combined_jsonl:find(session_id, 1, true), "JSONL must retain its local session boundary")
end

local notifications = {}
local invocations = {}
local snapshot_payloads = {}
local opened = 0
local workflow = report.new({
  analyzer = analyzer,
  output_directory = report_directory,
  session_directory = session_directory,
}, {
  notify = function(message)
    table.insert(notifications, message)
  end,
  open_file = function()
    opened = opened + 1
  end,
  run = function(argv, on_exit, stdin)
    table.insert(invocations, vim.deepcopy(argv))
    table.insert(snapshot_payloads, stdin)
    return process.run(argv, on_exit, stdin)
  end,
})

local function run_report()
  assert(workflow:start(), "the report workflow must launch the analyzer")
  assert(vim.wait(10000, function()
    return not workflow:status().running
  end), "the analyzer did not complete within the E2E timeout")
end

run_report()
assert(opened == 1, "a successful workflow must open the validated report")
local summary_path = vim.fs.joinpath(report_directory, "summary.json")
local report_path = vim.fs.joinpath(report_directory, "report.md")
local summary_json = table.concat(vim.fn.readfile(summary_path, "b"), "\n")
local report_markdown = table.concat(vim.fn.readfile(report_path, "b"), "\n")
local summary = vim.json.decode(summary_json)
assert(summary.schema_version == 3)
assert(summary.mapping_attribution.snapshot_version == 1)
assert(summary.sessions == 2)
assert(summary.text_runs == 1)
local j_count = nil
for _, key in ipairs(summary.keys) do
  if key.key == "j" then
    j_count = key.count
  end
end
assert(j_count == 2)
local attribution_by_id = {}
for _, mapping in ipairs(summary.mapping_attribution.mappings) do
  attribution_by_id[mapping.mapping_id] = mapping
end
assert(attribution_by_id[mapping_ids.global].status == "observed")
assert(attribution_by_id[mapping_ids.global].scope == "global")
assert(attribution_by_id[mapping_ids.global].count == 1)
assert(attribution_by_id[mapping_ids.removed].status == "observed_not_in_snapshot")
assert(attribution_by_id[mapping_ids.removed].scope == nil)
assert(attribution_by_id[mapping_ids.removed].count == 1)
assert(attribution_by_id[mapping_ids.buffer].status == "observed")
assert(attribution_by_id[mapping_ids.buffer].scope == "buffer")
assert(attribution_by_id[mapping_ids.buffer].count == 1)
assert(attribution_by_id[mapping_ids.collision_buffer].status == "observed")
assert(attribution_by_id[mapping_ids.collision_buffer].scope == "buffer")
assert(attribution_by_id[mapping_ids.collision_buffer].count == 1)
assert(attribution_by_id[mapping_ids.collision_global].status == "unobserved_in_sample")
assert(attribution_by_id[mapping_ids.collision_global].scope == "global")
assert(attribution_by_id[mapping_ids.collision_global].count == 0)
local expected_collision = false
for _, collision in ipairs(summary.mapping_attribution.collisions) do
  if collision.global_mapping_id == mapping_ids.collision_global
    and collision.buffer_mapping_id == mapping_ids.collision_buffer
  then
    expected_collision = true
  end
end
assert(expected_collision, "the exact global/buffer shadowing pair must be reported")

local local_only = vim.list_extend(vim.deepcopy(forbidden), session_ids)
assert_absent("summary.json", summary_json, local_only)
assert_absent("report.md", report_markdown, local_only)
assert_absent("notifications", table.concat(notifications, "\n"), local_only)
assert_absent("analyzer argv", table.concat(invocations[1], "\n"), local_only)
assert_absent("snapshot stdin", snapshot_payloads[1], local_only)

run_report()
assert(opened == 2)
assert(summary_json == table.concat(vim.fn.readfile(summary_path, "b"), "\n"))
assert(report_markdown == table.concat(vim.fn.readfile(report_path, "b"), "\n"))
assert(#invocations == 2)

for _, lhs in ipairs({ "z1", "z4" }) do
  vim.keymap.del("n", lhs)
end
for _, lhs in ipairs({ "z3", "z4" }) do
  vim.keymap.del("n", lhs, { buffer = current_buffer_id })
end
vim.api.nvim_buf_set_name(current_buffer_id, original_buffer_name)

vim.fn.delete(root, "rf")
print("Headless local workflow E2E: ok")
