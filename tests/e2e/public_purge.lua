local session_directory = assert(os.getenv("KEY_INSIGHTS_SESSION_DIR"))
local trace_path = assert(os.getenv("KEY_INSIGHTS_TRACE_PATH"))

local purge = require("key-insights.purge")
local original_new = purge.new
purge.new = function(options, dependencies)
  local instance = original_new(options, dependencies)
  instance._confirm = function()
    return false
  end
  return instance
end

local notifications = {}
vim.notify = function(message)
  table.insert(notifications, tostring(message))
end
local api = require("key-insights").setup({ storage = { directory = session_directory } })
vim.cmd.KeyInsightsStart()
local active_session = assert(api.status().session_id)

local function names()
  local result = {}
  local request = assert(vim.uv.fs_scandir(session_directory))
  while true do
    local name = vim.uv.fs_scandir_next(request)
    if name == nil then
      break
    end
    table.insert(result, name)
  end
  table.sort(result)
  return result
end

local before = names()
vim.cmd.KeyInsightsPurge()
local after_cancel = names()
vim.cmd("KeyInsightsPurge!")
local after_force = names()
vim.cmd.KeyInsightsStop()
local after_stop = names()
vim.fn.writefile({ vim.json.encode({
  after_cancel = after_cancel,
  after_force = after_force,
  after_stop = after_stop,
  active_session = active_session,
  before = before,
  notifications = notifications,
}) }, trace_path)
