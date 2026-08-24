local session_directory = assert(os.getenv("KEY_INSIGHTS_SESSION_DIR"))
local report_directory = assert(os.getenv("KEY_INSIGHTS_REPORT_DIR"))
local trace_path = assert(os.getenv("KEY_INSIGHTS_TRACE_PATH"))
local analyzer = assert(os.getenv("KEY_INSIGHTS_BIN"))

local notifications = {}
vim.notify = function(message)
  table.insert(notifications, tostring(message))
end

local api = require("key-insights").setup({
  storage = { directory = session_directory },
  report = { analyzer = analyzer, directory = report_directory },
})
vim.api.nvim_buf_set_lines(0, 0, -1, false, { "one", "two", "three", "four" })
vim.api.nvim_buf_set_name(0, "/private/PUBLIC_BUFFER_PATH_SECRET/source.lua")
vim.bo.filetype = "lua"
vim.keymap.set("n", "z9", "j", { desc = "PUBLIC_MAPPING_RHS_SECRET" })

local function feed(keys)
  vim.api.nvim_feedkeys(vim.api.nvim_replace_termcodes(keys, true, false, true), "xt", false)
  assert(vim.wait(1000, function()
    return api.status().pending_events == 0
  end), "collector did not flush public input")
end

vim.cmd.KeyInsightsStart()
local first_session = assert(api.status().session_id)
feed("jj")
feed("iPUBLIC_INSERT_TEXT_SECRET<Esc>")
feed(":echo 'PUBLIC_COMMAND_TEXT_SECRET'<Esc>")
feed("/PUBLIC_SEARCH_TEXT_SECRET<Esc>")
vim.cmd.KeyInsightsPause()
assert(api.status().state == "paused")
feed("kk")
assert(api.status().session_id == first_session)
vim.cmd.KeyInsightsStart()
assert(api.status().state == "recording")
assert(api.status().session_id == first_session)
feed("ll")
vim.cmd.KeyInsightsStatus()
vim.cmd.KeyInsightsStop()
assert(api.status().state == "stopped")

vim.cmd.KeyInsightsStart()
local second_session = assert(api.status().session_id)
assert(second_session ~= first_session)
feed("h")
vim.cmd.KeyInsightsStop()

vim.cmd.KeyInsightsReport()
assert(vim.wait(10000, function()
  return not api.status().report_running
end), "public report command did not finish")
local report_path = vim.fs.joinpath(report_directory, "report.md")
assert(vim.uv.fs_lstat(report_path) ~= nil, "public report command did not publish report.md")
vim.cmd.KeyInsightsOpenReport()
assert(
  vim.uv.fs_realpath(vim.api.nvim_buf_get_name(0)) == vim.uv.fs_realpath(report_path),
  "public open-report command did not open report.md"
)
vim.keymap.del("n", "z9")

vim.fn.writefile({ vim.json.encode({
  first_session = first_session,
  second_session = second_session,
  notifications = notifications,
  report_path = vim.api.nvim_buf_get_name(0),
  state = api.status().state,
}) }, trace_path)
