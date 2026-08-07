local collector = require("key-insights.collector")
local commands = require("key-insights.commands")
local config = require("key-insights.config")
local purge = require("key-insights.purge")
local report = require("key-insights.report")
local storage = require("key-insights.storage")

local M = {}
local options = config.defaults()
local instance = nil
local registered = false
local purge_instance = nil
local report_instance = nil
local writer = nil

local function get_writer()
  if writer == nil then
    writer = storage.new(options.storage)
  end
  return writer
end

local function get_instance()
  if instance == nil then
    instance = collector.new({
      options = options,
      open_session = function(session_id)
        return get_writer():open_session(session_id)
      end,
    })
  end
  return instance
end

local function get_report_instance()
  if report_instance == nil then
    local output_directory = options.report.directory or report.default_directory()
    report_instance = report.new({
      analyzer = options.report.analyzer,
      collector_options = options,
      output_directory = output_directory,
      session_directory = get_writer().directory,
    })
  end
  return report_instance
end

local function get_purge_instance()
  if purge_instance == nil then
    local current_writer = get_writer()
    purge_instance = purge.new({
      active_session_id = function()
        return instance == nil and nil or instance:status().session_id
      end,
      directory = current_writer.directory,
      include_legacy = current_writer:includes_legacy_logs(),
    })
  end
  return purge_instance
end

function M.setup(user_options)
  if instance ~= nil and instance:status().state ~= "stopped" then
    error("key-insights cannot be reconfigured during an active session")
  end
  if report_instance ~= nil and report_instance:status().running then
    error("key-insights cannot be reconfigured while a report is running")
  end
  options = config.resolve(user_options)
  instance = nil
  purge_instance = nil
  report_instance = nil
  writer = nil
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
  local status = instance == nil
      and { state = "stopped", session_id = nil, pending_events = 0, last_error = nil }
    or instance:status()
  status.report_running = report_instance ~= nil and report_instance:status().running
  return status
end

function M.report()
  return get_report_instance():start()
end

function M.open_report()
  return get_report_instance():open()
end

function M.purge(force)
  if report_instance ~= nil and report_instance:status().running then
    vim.notify("key-insights: cannot purge while a report is running", vim.log.levels.WARN)
    return nil
  end
  local ok, result = pcall(function()
    return get_purge_instance():run(force == true)
  end)
  if not ok then
    vim.notify("key-insights: purge failed: " .. tostring(result), vim.log.levels.ERROR)
    return nil
  end
  return result
end

function M.register_commands()
  if registered then
    return
  end
  commands.register(M)
  local group = vim.api.nvim_create_augroup("key-insights.lifecycle", { clear = true })
  vim.api.nvim_create_autocmd("VimLeavePre", {
    group = group,
    callback = function()
      if report_instance ~= nil and type(report_instance.shutdown) == "function" then
        pcall(report_instance.shutdown, report_instance)
      end
      local ok, error_message = pcall(M.stop)
      if not ok then
        vim.notify("key-insights failed to close its session: " .. tostring(error_message), vim.log.levels.ERROR)
      end
    end,
    desc = "Close the key-insights session before Neovim exits",
  })
  registered = true
end

return M
