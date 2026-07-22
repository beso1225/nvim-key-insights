local config = require("key-insights.config")
local schema = require("key-insights.schema")

local M = {}
local Collector = {}
Collector.__index = Collector

local function default_clock_ms()
  return math.floor(vim.uv.hrtime() / 1000000)
end

local function default_session_id()
  local entropy = table.concat({
    tostring(vim.uv.hrtime()),
    tostring(vim.fn.getpid()),
    tostring(math.random()),
  }, ":")
  return string.sub(vim.fn.sha256(entropy), 1, 32)
end

local function default_buffer()
  local buffer = vim.api.nvim_get_current_buf()
  return {
    buftype = vim.bo[buffer].buftype,
    filetype = vim.bo[buffer].filetype,
    name = vim.api.nvim_buf_get_name(buffer),
  }
end

local function default_register_on_key(callback)
  local namespace = vim.api.nvim_create_namespace("key-insights.collector")
  vim.on_key(callback, namespace)
  return function()
    vim.on_key(nil, namespace)
  end
end

local function default_open_session(_session_id)
  error("collector storage is not configured")
end

function M.new(spec)
  local dependencies = spec or {}
  local instance = setmetatable({
    _auto_flush = dependencies.auto_flush ~= false,
    _clock_ms = dependencies.clock_ms or default_clock_ms,
    _current_buffer = dependencies.current_buffer or default_buffer,
    _new_session_id = dependencies.new_session_id or default_session_id,
    _open_session = dependencies.open_session or default_open_session,
    _options = dependencies.options or config.defaults(),
    _pending = {},
    _register_on_key = dependencies.register_on_key or default_register_on_key,
    _started_at_ms = nil,
    _state = "stopped",
    _session_id = nil,
    _session_writer = nil,
    _end_queued = false,
    _unregister = nil,
    _last_error = nil,
  }, Collector)
  return instance
end

function Collector:_elapsed_ms()
  if self._started_at_ms == nil then
    return 0
  end
  return math.max(0, math.floor(self._clock_ms() - self._started_at_ms))
end

function Collector:_queue(event)
  table.insert(self._pending, schema.encode(event))
  if self._auto_flush then
    self:_write_pending()
  end
end

function Collector:_write_pending()
  if #self._pending == 0 then
    return 0
  end
  assert(self._session_writer ~= nil, "collector session storage is not open")

  local pending = self._pending
  self._session_writer:write(pending)
  self._pending = {}
  return #pending
end

function Collector:_is_excluded()
  local buffer = self._current_buffer()
  return config.is_excluded_buffer(buffer, self._options) or config.is_sensitive_buffer(buffer)
end

function Collector:_handle_key(_mapped, _typed)
  if self._state ~= "recording" or self:_is_excluded() then
    return
  end

  -- Sequence aggregation is intentionally added in a later slice. Keeping
  -- this callback installed now exercises lifecycle and privacy boundaries
  -- without persisting raw input.
end

function Collector:_attach()
  if self._unregister ~= nil then
    return
  end

  self._unregister = self._register_on_key(function(mapped, typed)
    local ok, error_message = pcall(self._handle_key, self, mapped, typed)
    if not ok then
      self._last_error = tostring(error_message)
    end
    return nil
  end)
end

function Collector:_detach()
  if self._unregister == nil then
    return
  end
  self._unregister()
  self._unregister = nil
end

function Collector:_reset_session()
  self._end_queued = false
  self._pending = {}
  self._session_id = nil
  self._session_writer = nil
  self._started_at_ms = nil
  self._state = "stopped"
end

function Collector:start()
  if self._state == "recording" then
    return false
  end

  if self._state == "starting" or self._state == "stopping" then
    error("collector lifecycle transition is already in progress")
  end

  if self._state == "paused" then
    local ok, error_message = pcall(self._attach, self)
    if not ok then
      self._last_error = tostring(error_message)
      error(error_message, 0)
    end
    self._state = "recording"
    self._last_error = nil
    return true
  end

  local session_id = self._new_session_id()
  local session_writer = self._open_session(session_id)
  self._session_id = session_id
  self._session_writer = session_writer
  self._started_at_ms = self._clock_ms()
  self._state = "starting"

  local ok, error_message = pcall(function()
    self:_queue(schema.session_start(self._session_id))
    self:flush()
    self:_attach()
  end)
  if not ok then
    self:_detach()
    pcall(session_writer.abort, session_writer)
    self:_reset_session()
    self._last_error = tostring(error_message)
    error(error_message, 0)
  end

  self._state = "recording"
  self._last_error = nil
  return true
end

function Collector:pause()
  if self._state ~= "recording" then
    return false
  end
  self:_detach()
  self._state = "paused"
  local ok, error_message = pcall(self.flush, self)
  if not ok then
    self._last_error = tostring(error_message)
    error(error_message, 0)
  end
  return true
end

function Collector:stop()
  if self._state == "stopped" then
    return false
  end

  self._state = "stopping"
  local ok, error_message = pcall(function()
    self:_detach()
    if not self._end_queued then
      self._end_queued = true
      self:_queue(schema.session_end(self._session_id, self:_elapsed_ms()))
    end
    self:flush()
    self._session_writer:finish()
  end)
  if not ok then
    self._last_error = tostring(error_message)
    error(error_message, 0)
  end

  self:_reset_session()
  self._last_error = nil
  return true
end

function Collector:flush()
  if self._session_writer == nil then
    return 0
  end

  local count = self:_write_pending()
  self._session_writer:flush()
  return count
end

function Collector:status()
  return {
    state = self._state,
    session_id = self._session_id,
    pending_events = #self._pending,
    last_error = self._last_error,
  }
end

return M
