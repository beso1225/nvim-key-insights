local session_directory = assert(os.getenv("KEY_INSIGHTS_SESSION_DIR"))
local report_directory = assert(os.getenv("KEY_INSIGHTS_REPORT_DIR"))
local analyzer = assert(os.getenv("KEY_INSIGHTS_BIN"))
local exit_state = assert(os.getenv("KEY_INSIGHTS_EXIT_STATE"))

local api = require("key-insights").setup({
  storage = { directory = session_directory },
  report = { analyzer = analyzer, directory = report_directory },
})
vim.api.nvim_buf_set_lines(0, 0, -1, false, { "one", "two", "three" })
vim.cmd.KeyInsightsStart()
vim.api.nvim_feedkeys("j", "xt", false)
assert(vim.wait(1000, function()
  return api.status().pending_events == 0
end), "collector did not flush before exit")
if exit_state == "paused" then
  vim.cmd.KeyInsightsPause()
  assert(api.status().state == "paused")
else
  assert(exit_state == "recording")
end
vim.cmd("qa!")
