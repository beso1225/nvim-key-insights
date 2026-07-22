local collector = require("key-insights.collector")
local commands = require("key-insights.commands")
local config = require("key-insights.config")
local storage = require("key-insights.storage")

local M = {}
local options = config.defaults()
local instance = nil

local function get_instance()
  if instance == nil then
    local writer = storage.new(options.storage)
    instance = collector.new({
      options = options,
      open_session = function(session_id)
        return writer:open_session(session_id)
      end,
    })
  end
  return instance
end

function M.setup(user_options)
  if instance ~= nil and instance:status().state ~= "stopped" then
    error("key-insights cannot be reconfigured during an active session")
  end
  options = config.resolve(user_options)
  instance = nil
  M.register_commands()
  return M
end

function M.start()
  return get_instance():start()
end

function M.pause()
  return get_instance():pause()
end

function M.stop()
  if instance == nil then
    return false
  end
  return instance:stop()
end

function M.flush()
  if instance == nil then
    return 0
  end
  return instance:flush()
end

function M.status()
  if instance == nil then
    return { state = "stopped", session_id = nil, pending_events = 0, last_error = nil }
  end
  return instance:status()
end

function M.register_commands()
  commands.register(M)
  local group = vim.api.nvim_create_augroup("key-insights.lifecycle", { clear = true })
  vim.api.nvim_create_autocmd("VimLeavePre", {
    group = group,
    callback = function()
      local ok, error_message = pcall(M.stop)
      if not ok then
        vim.notify("key-insights failed to close its session: " .. tostring(error_message), vim.log.levels.ERROR)
      end
    end,
    desc = "Close the key-insights session before Neovim exits",
  })
end

return M
