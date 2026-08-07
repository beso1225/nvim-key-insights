local config = require("key-insights.config")
local key_tokens = require("key-insights.key_tokens")
local mapping_attribution = require("key-insights.mapping_attribution")
local mapping_resolver = require("key-insights.mapping_resolver")
local schema = require("key-insights.schema")

local M = {}
local Collector = {}
Collector.__index = Collector

local SEQUENCE_MODES = {
  normal = true,
  operator_pending = true,
  visual = true,
}
local MAX_CALLBACK_INPUT_BYTES = schema.MAX_EVENT_LINE_BYTES * 4
local MAX_PENDING_EVENTS = 1024
local MAX_PENDING_BYTES = 4 * 1024 * 1024
local PENDING_LIMIT_ERROR = "collector pending queue limit exceeded"
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
    id = buffer,
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

local function default_current_mode()
  return vim.api.nvim_get_mode().mode
end

local function default_current_cmdtype()
  return vim.fn.getcmdtype()
end

local function default_keytrans(key)
  return vim.fn.keytrans(key)
end

local function normalize_mode(mode, cmdtype)
  if string.sub(mode, 1, 2) == "no" then
    return "operator_pending"
  end
  if string.sub(mode, 1, 1) == "n" then
    return "normal"
  end
  if mode == "v" or mode == "V" or mode == "\22" then
    return "visual"
  end
  local prefix = string.sub(mode, 1, 1)
  if prefix == "i" or prefix == "R" or mode == "s" or mode == "S" or mode == "\19" then
    return "insert"
  end
  if prefix == "c" then
    if cmdtype == "/" or cmdtype == "?" then
      return "search"
    end
    return "command"
  end
  return "other"
end

function M.new(spec)
  local dependencies = spec or {}
  local options = dependencies.options or config.defaults()
  local resolver = dependencies.mapping_resolver
  if resolver == nil then
    resolver = mapping_resolver.new()
  end
  local instance = setmetatable({
    _auto_flush = dependencies.auto_flush ~= false,
    _clock_ms = dependencies.clock_ms or default_clock_ms,
    _current_buffer = dependencies.current_buffer or default_buffer,
    _current_cmdtype = dependencies.current_cmdtype or default_current_cmdtype,
    _current_mode = dependencies.current_mode or default_current_mode,
    _flush_epoch = 0,
    _flush_scheduled = false,
    _in_callback = false,
    _keytrans = dependencies.keytrans or default_keytrans,
    _last_mode = nil,
    _mapping_resolver = resolver,
    _mapping_ready = false,
    _new_session_id = dependencies.new_session_id or default_session_id,
    _open_session = dependencies.open_session or default_open_session,
    _options = options,
    _pending = {},
    _pending_bytes = 0,
    _pending_byte_limit = dependencies.pending_byte_limit or MAX_PENDING_BYTES,
    _pending_event_limit = dependencies.pending_event_limit or MAX_PENDING_EVENTS,
    _register_on_key = dependencies.register_on_key or default_register_on_key,
    _schedule = dependencies.schedule or vim.schedule,
    _started_at_ms = nil,
    _state = "stopped",
    _session_id = nil,
    _session_writer = nil,
    _sequence = nil,
    _text_run = nil,
    _end_queued = false,
    _unregister = nil,
    _last_error = nil,
  }, Collector)
  return instance
end

function Collector:_emit_sequence(elapsed_ms)
  local sequence = self._sequence
  if sequence == nil then
    return
  end
  self._sequence = nil

  local events = {}
  local chunk_keys = {}
  local chunk_size = 0
  local chunk_started_ms = nil
  local chunk_last_ms = nil

  local function finish_chunk()
    if #chunk_keys == 0 then
      return
    end
    local event = schema.key_sequence(
      self._session_id,
      elapsed_ms,
      sequence.mode,
      chunk_keys,
      chunk_last_ms - chunk_started_ms
    )
    assert(#schema.encode(event) <= schema.MAX_EVENT_LINE_BYTES, "key sequence exceeds the event line limit")
    table.insert(events, event)
    chunk_keys = {}
    chunk_size = 0
    chunk_started_ms = nil
    chunk_last_ms = nil
  end

  for index, key in ipairs(sequence.keys) do
    local key_elapsed_ms = sequence.key_elapsed_ms[index]
    if #chunk_keys == 0 then
      local event = schema.key_sequence(self._session_id, elapsed_ms, sequence.mode, { key }, 0)
      chunk_size = #schema.encode(event)
      assert(chunk_size <= schema.MAX_EVENT_LINE_BYTES, "key token exceeds the event line limit")
      chunk_started_ms = key_elapsed_ms
      chunk_last_ms = key_elapsed_ms
      table.insert(chunk_keys, key)
    else
      local old_duration_ms = chunk_last_ms - chunk_started_ms
      local new_duration_ms = key_elapsed_ms - chunk_started_ms
      local candidate_size = chunk_size
        + 1
        + #vim.json.encode(key)
        + #tostring(new_duration_ms)
        - #tostring(old_duration_ms)
      if candidate_size > schema.MAX_EVENT_LINE_BYTES then
        finish_chunk()
        local event = schema.key_sequence(self._session_id, elapsed_ms, sequence.mode, { key }, 0)
        chunk_size = #schema.encode(event)
        assert(chunk_size <= schema.MAX_EVENT_LINE_BYTES, "key token exceeds the event line limit")
        chunk_started_ms = key_elapsed_ms
        chunk_last_ms = key_elapsed_ms
        table.insert(chunk_keys, key)
      else
        table.insert(chunk_keys, key)
        chunk_size = candidate_size
        chunk_last_ms = key_elapsed_ms
      end
    end
  end
  finish_chunk()
  self:_queue_many(events)
end

function Collector:_emit_text_run(elapsed_ms)
  local text_run = self._text_run
  if text_run == nil then
    return
  end
  self._text_run = nil
  self:_queue(schema.text_run(
    self._session_id,
    elapsed_ms,
    text_run.key_count,
    text_run.last_ms - text_run.started_ms
  ))
end

function Collector:_flush_input(elapsed_ms)
  self:_emit_sequence(elapsed_ms)
  self:_emit_text_run(elapsed_ms)
end

function Collector:_typed_tokens(typed)
  if type(typed) ~= "string" or typed == "" or #typed > MAX_CALLBACK_INPUT_BYTES then
    return {}
  end
  local canonical = self._keytrans(typed)
  if type(canonical) ~= "string" or canonical == "" then
    return {}
  end

  local tokens = key_tokens.tokenize(canonical, {
    max_input_bytes = MAX_CALLBACK_INPUT_BYTES,
    max_token_bytes = 256,
    max_tokens = MAX_CALLBACK_INPUT_BYTES,
  })
  return tokens or {}
end

function Collector:_record_sequence(mode, typed, elapsed_ms, typed_tokens)
  for _, key in ipairs(typed_tokens or self:_typed_tokens(typed)) do
    local sequence = self._sequence
    local timeout_ms = self._options.collection.sequence_timeout_ms
    local max_keys = self._options.collection.max_sequence_keys
    if sequence ~= nil
      and (sequence.mode ~= mode or elapsed_ms - sequence.last_ms > timeout_ms or #sequence.keys >= max_keys)
    then
      self:_emit_sequence(elapsed_ms)
      sequence = nil
    end

    if sequence == nil then
      sequence = {
        key_elapsed_ms = {},
        keys = {},
        last_ms = elapsed_ms,
        mode = mode,
        started_ms = elapsed_ms,
      }
      self._sequence = sequence
    end
    table.insert(sequence.keys, key)
    table.insert(sequence.key_elapsed_ms, elapsed_ms)
    sequence.last_ms = elapsed_ms
  end
end

function Collector:_record_text_keys(typed, elapsed_ms)
  local key_count = #self:_typed_tokens(typed)
  if key_count == 0 then
    return
  end
  if self._text_run == nil then
    self._text_run = {
      key_count = 0,
      last_ms = elapsed_ms,
      started_ms = elapsed_ms,
    }
  end
  self._text_run.key_count = self._text_run.key_count + key_count
  self._text_run.last_ms = elapsed_ms
end

function Collector:_elapsed_ms()
  if self._started_at_ms == nil then
    return 0
  end
  return math.max(0, math.floor(self._clock_ms() - self._started_at_ms))
end

function Collector:_schedule_pending_write()
  if self._flush_scheduled then
    return
  end
  self._flush_scheduled = true
  local epoch = self._flush_epoch
  local scheduled = pcall(self._schedule, function()
    if epoch ~= self._flush_epoch then
      return
    end
    self._flush_scheduled = false
    if self._in_callback or self._state ~= "recording" or self._last_error ~= nil then
      return
    end
    local ok, error_message = pcall(self._write_pending, self)
    if not ok then
      self._last_error = tostring(error_message)
    end
  end)
  if not scheduled then
    self._flush_scheduled = false
  end
end

function Collector:_queue_many(events)
  local encoded_events = {}
  local encoded_bytes = 0
  for _, event in ipairs(events) do
    local encoded = schema.encode(event)
    table.insert(encoded_events, encoded)
    encoded_bytes = encoded_bytes + #encoded
  end
  if #self._pending + #encoded_events > self._pending_event_limit
    or self._pending_bytes + encoded_bytes > self._pending_byte_limit
  then
    self._last_error = PENDING_LIMIT_ERROR
    return false
  end
  for _, encoded in ipairs(encoded_events) do
    table.insert(self._pending, encoded)
  end
  self._pending_bytes = self._pending_bytes + encoded_bytes
  if self._auto_flush then
    if self._in_callback then
      self:_schedule_pending_write()
    else
      self:_write_pending()
    end
  end
  return true
end

function Collector:_queue(event)
  self:_queue_many({ event })
end

function Collector:_write_pending()
  if #self._pending == 0 then
    return 0
  end
  assert(self._session_writer ~= nil, "collector session storage is not open")

  local pending = self._pending
  self._session_writer:write(pending)
  self._pending = {}
  self._pending_bytes = 0
  return #pending
end

function Collector:_is_excluded()
  local buffer = self._current_buffer()
  return config.is_excluded_buffer(buffer, self._options) or config.is_sensitive_buffer(buffer)
end

function Collector:_mapping_boundary()
  if self._mapping_resolver == nil then
    return
  end
  local callback = self._mapping_resolver.boundary or self._mapping_resolver.reset
  if type(callback) == "function" then
    pcall(callback, self._mapping_resolver)
  end
end

function Collector:_prime_mapping_resolver()
  self._mapping_ready = false
  if self._mapping_resolver == nil or type(self._mapping_resolver.prime) ~= "function" then
    return false
  end
  local context_ok, buffer = pcall(self._current_buffer)
  if not context_ok or type(buffer) ~= "table" or type(buffer.id) ~= "number" then
    return false
  end
  local eligibility_ok, excluded = pcall(function()
    return config.is_excluded_buffer(buffer, self._options) or config.is_sensitive_buffer(buffer)
  end)
  if not eligibility_ok or excluded then
    return false
  end
  local ok, primed = pcall(self._mapping_resolver.prime, self._mapping_resolver, buffer.id)
  self._mapping_ready = ok and primed == true
  return self._mapping_ready
end

function Collector:_record_mapping_use(mapped, typed, mode, typed_tokens, elapsed_ms)
  if self._mapping_resolver == nil or not self._mapping_ready then
    return
  end
  local evidence = mapping_attribution.classify_callback(mapped, typed)
  if evidence ~= "typed_same" and evidence ~= "typed_different" then
    return
  end
  local ok, candidate = pcall(self._mapping_resolver.resolve, self._mapping_resolver, mode, typed_tokens)
  if not ok or type(candidate) ~= "table" then
    return
  end
  local mapping_id = rawget(candidate, "mapping_id")
  local candidate_mode = rawget(candidate, "mode")
  local scope = rawget(candidate, "scope")
  if candidate_mode ~= mode
    or (scope ~= "global" and scope ~= "buffer")
    or type(mapping_id) ~= "string"
    or #mapping_id ~= 75
    or string.match(mapping_id, "^mapping%-v1:[0-9a-f]+$") == nil
    or #typed_tokens == 0
  then
    return
  end
  self:_queue(schema.mapping_use(self._session_id, elapsed_ms, mode, mapping_id, typed_tokens))
end

function Collector:_handle_key(mapped, typed)
  if self._state ~= "recording" or self._last_error ~= nil then
    return
  end

  local elapsed_ms = self:_elapsed_ms()
  if self:_is_excluded() then
    self._last_mode = nil
    self:_mapping_boundary()
    self:_flush_input(elapsed_ms)
    return
  end

  local mode = normalize_mode(self._current_mode(), self._current_cmdtype())
  if self._last_mode ~= nil and self._last_mode ~= mode then
    local previous_mode = self._last_mode
    self._last_mode = mode
    self:_flush_input(elapsed_ms)
    self:_mapping_boundary()
    self:_queue(schema.mode_transition(self._session_id, elapsed_ms, previous_mode, mode))
  else
    if self._last_mode == nil then
      self:_mapping_boundary()
    end
    self._last_mode = mode
  end

  if SEQUENCE_MODES[mode] then
    local typed_tokens = self:_typed_tokens(typed)
    self:_record_sequence(mode, typed, elapsed_ms, typed_tokens)
    self:_record_mapping_use(mapped, typed, mode, typed_tokens, elapsed_ms)
  elseif mode == "insert" then
    self:_record_text_keys(typed, elapsed_ms)
  end
end

function Collector:_attach()
  if self._unregister ~= nil then
    return
  end

  self._unregister = self._register_on_key(function(mapped, typed)
    self._in_callback = true
    local ok, error_message = pcall(self._handle_key, self, mapped, typed)
    self._in_callback = false
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
  self._flush_epoch = self._flush_epoch + 1
  self._flush_scheduled = false
  self._in_callback = false
  self._end_queued = false
  self._pending = {}
  self._pending_bytes = 0
  self._last_mode = nil
  self._mapping_ready = false
  if self._mapping_resolver ~= nil and type(self._mapping_resolver.reset) == "function" then
    pcall(self._mapping_resolver.reset, self._mapping_resolver)
  end
  self._sequence = nil
  self._session_id = nil
  self._session_writer = nil
  self._started_at_ms = nil
  self._state = "stopped"
  self._text_run = nil
end

function Collector:start()
  if self._state == "recording" then
    return false
  end

  if self._state == "starting" or self._state == "stopping" then
    error("collector lifecycle transition is already in progress")
  end

  if self._state == "paused" then
    self:_prime_mapping_resolver()
    local ok, error_message = pcall(self._attach, self)
    if not ok then
      self._last_error = tostring(error_message)
      error(error_message, 0)
    end
    self._state = "recording"
    self._last_mode = nil
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
    self:_prime_mapping_resolver()
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
  self._mapping_ready = false
  if self._mapping_resolver ~= nil and type(self._mapping_resolver.reset) == "function" then
    pcall(self._mapping_resolver.reset, self._mapping_resolver)
  end
  local ok, error_message = pcall(self.flush, self)
  if not ok then
    self._last_error = tostring(error_message)
    error(error_message, 0)
  end
  self._last_mode = nil
  return true
end

function Collector:stop()
  if self._state == "stopped" then
    return false
  end

  self._state = "stopping"
  local ok, error_message = pcall(function()
    self:_detach()
    self._last_mode = nil
    self:_write_pending()
    if not self._end_queued then
      self:_flush_input(self:_elapsed_ms())
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

  self:_flush_input(self:_elapsed_ms())
  self._mapping_ready = false
  if self._mapping_resolver ~= nil and type(self._mapping_resolver.reset) == "function" then
    pcall(self._mapping_resolver.reset, self._mapping_resolver)
  end
  local count = self:_write_pending()
  self._session_writer:flush()
  if self._state == "recording" then
    self:_prime_mapping_resolver()
  end
  return count
end

function Collector:status()
  return {
    state = self._state,
    session_id = self._session_id,
    pending_events = #self._pending,
    pending_bytes = self._pending_bytes,
    last_error = self._last_error,
  }
end

return M
