local session_directory = assert(os.getenv("KEY_INSIGHTS_SESSION_DIR"))
local report_directory = assert(os.getenv("KEY_INSIGHTS_REPORT_DIR"))
local trace_path = assert(os.getenv("KEY_INSIGHTS_TRACE_PATH"))
local analyzer = assert(os.getenv("KEY_INSIGHTS_BIN"))
local failing_analyzer = assert(os.getenv("KEY_INSIGHTS_FAILING_ANALYZER"))

local notifications = {}
vim.notify = function(message)
  table.insert(notifications, tostring(message))
end

local api = require("key-insights").setup({
  storage = { directory = session_directory },
  report = { analyzer = analyzer, directory = report_directory },
})
vim.bo.filetype = "lua"
vim.cmd.KeyInsightsStart()
vim.api.nvim_feedkeys("j", "xt", false)
assert(vim.wait(1000, function()
  return api.status().pending_events == 0
end), "collector did not flush failure-path input")
vim.cmd.KeyInsightsStop()
vim.cmd.KeyInsightsReport()
assert(vim.wait(10000, function()
  return not api.status().report_running
end), "known-good report did not finish")

local summary_path = vim.fs.joinpath(report_directory, "summary.json")
local report_path = vim.fs.joinpath(report_directory, "report.md")
local summary_before = table.concat(vim.fn.readfile(summary_path, "b"), "\n")
local report_before = table.concat(vim.fn.readfile(report_path, "b"), "\n")

api = require("key-insights").setup({
  storage = { directory = session_directory },
  report = { analyzer = failing_analyzer, directory = report_directory },
})
vim.cmd.KeyInsightsReport()
assert(vim.wait(10000, function()
  return not api.status().report_running
end), "failed report did not settle")
vim.cmd.enew()
assert(vim.api.nvim_buf_get_name(0) == "", "OpenReport precondition must use a distinct scratch buffer")
vim.cmd.KeyInsightsOpenReport()
assert(vim.uv.fs_realpath(vim.api.nvim_buf_get_name(0)) == vim.uv.fs_realpath(report_path))
assert(
  table.concat(vim.api.nvim_buf_get_lines(0, 0, -1, false), "\n")
    == table.concat(vim.fn.readfile(report_path), "\n"),
  "OpenReport must reload the preserved report contents"
)

vim.fn.writefile({ vim.json.encode({
  notifications = notifications,
  report_preserved = report_before == table.concat(vim.fn.readfile(report_path, "b"), "\n"),
  summary_preserved = summary_before == table.concat(vim.fn.readfile(summary_path, "b"), "\n"),
}) }, trace_path)
