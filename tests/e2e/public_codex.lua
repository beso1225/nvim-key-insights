local session_directory = assert(os.getenv("KEY_INSIGHTS_SESSION_DIR"))
local report_directory = assert(os.getenv("KEY_INSIGHTS_REPORT_DIR"))
local trace_path = assert(os.getenv("KEY_INSIGHTS_TRACE_PATH"))
local preview_path = assert(os.getenv("KEY_INSIGHTS_PREVIEW_PATH"))
local suggestions_path = assert(os.getenv("KEY_INSIGHTS_SUGGESTIONS_PATH"))
local analyzer = assert(os.getenv("KEY_INSIGHTS_BIN"))
local codex_binary = assert(os.getenv("KEY_INSIGHTS_MOCK_CODEX"))

local notifications = {}
vim.notify = function(message)
  table.insert(notifications, tostring(message))
end

local confirmation_count = 0
local confirmation_prompt = nil
local preview_contents = nil
local approve = true
vim.ui.select = function(_, options, callback)
  confirmation_count = confirmation_count + 1
  confirmation_prompt = options.prompt
  local lines = vim.api.nvim_buf_get_lines(0, 0, -1, false)
  preview_contents = table.concat(lines, "\n")
  if approve then
    vim.fn.writefile({ "confirmed" }, vim.fs.joinpath(vim.fs.dirname(codex_binary), "confirmation-marker"))
  end
  callback(approve and "Run Codex analysis" or "Cancel")
end

local api = require("key-insights").setup({
  storage = { directory = session_directory },
  report = {
    analyzer = analyzer,
    directory = report_directory,
    codex = { binary = codex_binary },
  },
})
vim.api.nvim_buf_set_lines(0, 0, -1, false, { "one", "two", "three" })
vim.api.nvim_buf_set_name(0, "/private/CODEX_BUFFER_PATH_SECRET/source.lua")
vim.bo.filetype = "lua"
vim.keymap.set("n", "z8", "<Cmd>let g:CODEX_MAPPING_RHS_SECRET = 1<CR>", { desc = "Codex E2E mapping" })

local function feed(keys)
  vim.api.nvim_feedkeys(vim.api.nvim_replace_termcodes(keys, true, false, true), "xt", false)
  assert(vim.wait(1000, function()
    return api.status().pending_events == 0
  end), "collector did not flush Codex E2E input")
end

vim.cmd.KeyInsightsStart()
local session_id = assert(api.status().session_id)
feed("jjj")
feed("z8")
assert(vim.g.CODEX_MAPPING_RHS_SECRET == 1, "Codex mapping RHS canary did not execute")
feed("iCODEX_INSERT_TEXT_SECRET<Esc>")
feed("iCODEX_UNICODE_雪_\\\"_SECRET<Esc>")
feed(":echo 'CODEX_COMMAND_TEXT_SECRET'<Esc>")
feed("/CODEX_SEARCH_TEXT_SECRET<Esc>")
vim.cmd.KeyInsightsStop()
vim.cmd.KeyInsightsReport()
assert(vim.wait(10000, function()
  return not api.status().report_running
end), "Codex E2E report did not finish")
local report_path = vim.fs.joinpath(report_directory, "report.md")
vim.fn.writefile({ "CODEX_REPORT_ONLY_SECRET" }, report_path, "a")
vim.fn.writefile({ "CODEX_ADJACENT_FILE_SECRET" }, vim.fs.joinpath(report_directory, "private-notes.txt"))

vim.cmd.KeyInsightsAnalyze()
assert(vim.wait(10000, function()
  return not api.status().report_running
end), "mocked Codex analysis did not finish")
assert(vim.bo.filetype == "markdown", "validated suggestions must open as Markdown")
local suggestions_contents = table.concat(vim.api.nvim_buf_get_lines(0, 0, -1, false), "\n")
vim.fn.writefile(vim.split(assert(preview_contents), "\n", { plain = true }), preview_path, "b")
vim.fn.writefile(vim.split(suggestions_contents, "\n", { plain = true }), suggestions_path)

approve = false
vim.cmd.KeyInsightsAnalyze()
assert(vim.wait(10000, function()
  return not api.status().report_running
end), "cancelled Codex analysis did not settle")
assert(confirmation_count == 2)
vim.keymap.del("n", "z8")
vim.fn.writefile({ vim.json.encode({
  confirmation_count = confirmation_count,
  confirmation_prompt = confirmation_prompt,
  notifications = notifications,
  session_id = session_id,
  suggestions_filetype = "markdown",
}) }, trace_path)
