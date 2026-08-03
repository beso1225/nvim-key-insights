local collector = require("key-insights.collector")
local config = require("key-insights.config")
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

local instance = collector.new({
  clock_ms = function()
    return now_ms
  end,
  current_buffer = function()
    return {
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
  run = function(argv, on_exit)
    table.insert(invocations, vim.deepcopy(argv))
    return process.run(argv, on_exit)
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
assert(summary.schema_version == 1)
assert(summary.sessions == 2)
assert(summary.text_runs == 1)
assert(summary.keys[1].key == "j" and summary.keys[1].count == 2)

local local_only = vim.list_extend(vim.deepcopy(forbidden), session_ids)
assert_absent("summary.json", summary_json, local_only)
assert_absent("report.md", report_markdown, local_only)
assert_absent("notifications", table.concat(notifications, "\n"), local_only)
assert_absent("analyzer argv", table.concat(invocations[1], "\n"), local_only)

run_report()
assert(opened == 2)
assert(summary_json == table.concat(vim.fn.readfile(summary_path, "b"), "\n"))
assert(report_markdown == table.concat(vim.fn.readfile(report_path, "b"), "\n"))
assert(#invocations == 2)

vim.fn.delete(root, "rf")
print("Headless local workflow E2E: ok")
