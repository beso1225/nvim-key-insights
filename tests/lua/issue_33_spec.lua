local collector = require("key-insights.collector")
local config = require("key-insights.config")

local function new_harness(session_id, options, overrides)
  overrides = overrides or {}
  local state = {
    callback = nil,
    events = {},
    mode = overrides.mode or "n",
    now_ms = 0,
    scheduled = {},
  }
  local instance = collector.new({
    auto_flush = overrides.auto_flush,
    clock_ms = function()
      return state.now_ms
    end,
    current_buffer = function()
      return { id = 1, buftype = "", filetype = "lua", name = "issue-33.lua" }
    end,
    current_cmdtype = function()
      return ""
    end,
    current_mode = function()
      return state.mode
    end,
    keytrans = function(value)
      return value
    end,
    mapping_resolver = {
      prime = function()
        return false
      end,
      reset = function() end,
    },
    new_session_id = function()
      return session_id
    end,
    open_session = function()
      return {
        write = function(_, lines)
          for _, line in ipairs(lines) do
            table.insert(state.events, vim.json.decode(line))
          end
        end,
        flush = function() end,
        finish = function() end,
        abort = function() end,
      }
    end,
    options = options or config.defaults(),
    register_on_key = function(callback)
      state.callback = callback
      return function()
        state.callback = nil
      end
    end,
    schedule = function(callback)
      table.insert(state.scheduled, callback)
    end,
  })

  function state:drain()
    local scheduled = self.scheduled
    self.scheduled = {}
    for _, callback in ipairs(scheduled) do
      callback()
    end
  end

  assert(instance:start())
  return instance, state
end

local function events_of_type(events, event_type)
  local result = {}
  for _, event in ipairs(events) do
    if event.event_type == event_type then
      table.insert(result, event)
    end
  end
  return result
end

local function assert_sequence_count(label, mode, typed, expected_count)
  local instance, state = new_harness(
    "issue-33-" .. label,
    config.resolve({ collection = { max_sequence_keys = 65536 } }),
    { mode = mode }
  )
  state.callback("mapped-RHS-must-not-persist", typed)
  assert(instance:pause())

  local sequences = events_of_type(state.events, "key_sequence")
  local key_count = 0
  for _, event in ipairs(sequences) do
    key_count = key_count + #event.keys
    assert(#vim.json.encode(event) + 1 <= 64 * 1024)
  end
  assert(key_count == expected_count, label .. " must preserve every typed key")
  assert(string.find(vim.json.encode(state.events), "mapped-RHS", 1, true) == nil)
end

-- 90,000 three-byte characters exceed the callback input ceiling while still
-- remaining a bounded, deterministic test fixture.
local oversized_unicode = string.rep("日", 90000)
assert_sequence_count("normal", "n", oversized_unicode, 90000)
assert_sequence_count("visual", "v", oversized_unicode, 90000)
assert_sequence_count("operator-pending", "no", oversized_unicode, 90000)

local insert, insert_state = new_harness("issue-33-insert", config.defaults(), { mode = "i" })
insert_state.callback("mapped-insert-secret", oversized_unicode)
assert(insert:pause())
local text_runs = events_of_type(insert_state.events, "text_run")
assert(#text_runs == 1)
assert(text_runs[1].key_count == 90000, "oversized Insert input must preserve its key count")
assert(string.find(vim.json.encode(insert_state.events), "mapped-insert-secret", 1, true) == nil)

local control_options = config.resolve({ privacy = { capture_control_keys = true } })
local control, control_state = new_harness("issue-33-control", control_options, { mode = "i" })
local oversized_controls = string.rep("<C-Y>", 60000)
control_state.callback("mapped-control-secret", oversized_controls)
assert(control:pause())
local control_runs = events_of_type(control_state.events, "text_run")
assert(#control_runs == 1 and control_runs[1].key_count == 60000)
local control_uses = events_of_type(control_state.events, "control_key_use")
assert(#control_uses == 1)
assert(control_uses[1].key == "<C-Y>" and control_uses[1].count == 60000)
assert(string.find(vim.json.encode(control_state.events), "mapped-control-secret", 1, true) == nil)

local overflow, overflow_state = new_harness(
  "issue-33-queue-overflow",
  config.resolve({ collection = { max_sequence_keys = 1 } }),
  { auto_flush = true }
)
for index = 1, 2000 do
  overflow_state.now_ms = index
  overflow_state.callback("mapped-overflow-secret", "j")
end
local overflow_status = overflow:status()
assert(overflow_status.pending_events <= 1024)
assert(overflow_status.pending_bytes <= 4 * 1024 * 1024)
assert(overflow_status.last_error == "collector pending queue limit exceeded")
assert(overflow:stop())

local losses = events_of_type(overflow_state.events, "input_loss")
assert(#losses == 1, "queue overflow must persist one sanitized loss event")
assert(losses[1].reason == "pending_queue_limit")
assert(losses[1].key_count > 0)
local session_end_index = #overflow_state.events
for index, event in ipairs(overflow_state.events) do
  if event.event_type == "session_end" then
    session_end_index = index
  end
end
local loss_index = 0
for index, event in ipairs(overflow_state.events) do
  if event.event_type == "input_loss" then
    loss_index = index
  end
end
assert(loss_index > 0 and loss_index < session_end_index)
assert(string.find(vim.json.encode(overflow_state.events), "mapped-overflow-secret", 1, true) == nil)

local byte_overflow, byte_state = new_harness(
  "issue-33-byte-overflow",
  config.resolve({ collection = { max_sequence_keys = 20000 } }),
  { auto_flush = true }
)
local large_sequence = string.rep("j", 20000)
for index = 1, 70 do
  byte_state.now_ms = index * 1001
  byte_state.callback("mapped-byte-overflow-secret", large_sequence)
end
local byte_status = byte_overflow:status()
assert(byte_status.pending_events < 1024)
assert(byte_status.pending_bytes <= 4 * 1024 * 1024)
assert(byte_status.last_error == "collector pending queue limit exceeded")
assert(byte_overflow:stop())
local byte_losses = events_of_type(byte_state.events, "input_loss")
assert(#byte_losses == 1 and byte_losses[1].key_count > 0)
assert(string.find(vim.json.encode(byte_state.events), "mapped-byte-overflow-secret", 1, true) == nil)

print("Lua issue #33 contract: ok")
