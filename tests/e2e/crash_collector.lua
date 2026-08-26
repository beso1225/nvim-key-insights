local session_directory = assert(os.getenv("KEY_INSIGHTS_SESSION_DIR"))
local report_directory = assert(os.getenv("KEY_INSIGHTS_REPORT_DIR"))
local ready_path = assert(os.getenv("KEY_INSIGHTS_READY_PATH"))
local analyzer = assert(os.getenv("KEY_INSIGHTS_BIN"))

local api = require("key-insights").setup({
  storage = { directory = session_directory },
  report = { analyzer = analyzer, directory = report_directory },
})
vim.api.nvim_buf_set_lines(0, 0, -1, false, { "one", "two", "three" })
vim.cmd.KeyInsightsStart()
vim.api.nvim_feedkeys("j", "xt", false)
assert(vim.wait(1000, function()
  return api.status().pending_events == 0
end), "collector did not flush before crash marker")
vim.fn.writefile({ assert(api.status().session_id) }, ready_path)
vim.wait(30000, function()
  return false
end)
error("crash collector was not terminated by its parent")
