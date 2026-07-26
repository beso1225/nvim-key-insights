local collector = require("key-insights.collector")
local config = require("key-insights.config")
local schema = require("key-insights.schema")
local storage = require("key-insights.storage")

local function new_harness(session_id, options)
  local state = {
    buffer = { buftype = "", filetype = "lua", name = "src/init.lua" },
    callback = nil,
    cmdtype = "",
    events = {},
    mode = "n",
    now_ms = 0,
  }

  local instance = collector.new({
    clock_ms = function()
      return state.now_ms
    end,
    current_buffer = function()
      return state.buffer
    end,
    current_cmdtype = function()
      return state.cmdtype
    end,
    current_mode = function()
      return state.mode
    end,
    keytrans = function(key)
      return key
    end,
    new_session_id = function()
      return session_id
    end,
    options = options,
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
    register_on_key = function(callback)
      state.callback = callback
      return function()
        state.callback = nil
      end
    end,
  })

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

local sequence_collector, sequence = new_harness("aggregation-sequence")
sequence_collector:start()
sequence.now_ms = 10
assert(sequence.callback("mapped-rhs-secret", "gg") == nil)
sequence.now_ms = 20
sequence.callback("another-mapped-secret", "d")
sequence.now_ms = 25
sequence.callback("mapped-expansion-secret", "")
sequence.now_ms = 30
sequence_collector:pause()

local sequences = events_of_type(sequence.events, "key_sequence")
assert(#sequences == 1)
assert(vim.deep_equal(sequences[1].keys, { "g", "g", "d" }))
assert(sequences[1].mode == "normal")
assert(sequences[1].duration_ms == 10)
assert(sequences[1].elapsed_ms == 30)
local sequence_json = vim.json.encode(sequence.events)
assert(string.find(sequence_json, "mapped%-rhs%-secret") == nil, "mapping RHS must never enter sequence logs")
assert(string.find(sequence_json, "another%-mapped%-secret") == nil, "mapping expansion must remain private")
assert(string.find(sequence_json, "mapped%-expansion%-secret") == nil, "untyped mapping expansion must be ignored")

local timeout_collector, timeout = new_harness("aggregation-timeout")
timeout_collector:start()
timeout.now_ms = 100
timeout.callback("j", "j")
timeout.now_ms = 1201
timeout.callback("k", "k")
timeout.now_ms = 1300
timeout_collector:stop()

local timeout_sequences = events_of_type(timeout.events, "key_sequence")
assert(#timeout_sequences == 2, "inter-key timeout must split sequences")
assert(vim.deep_equal(timeout_sequences[1].keys, { "j" }))
assert(vim.deep_equal(timeout_sequences[2].keys, { "k" }))

local bounded_collector, bounded = new_harness(
  "aggregation-bounded",
  config.resolve({ collection = { max_sequence_keys = 2 } })
)
bounded_collector:start()
bounded.now_ms = 10
bounded.callback("mapped-bounded-secret", "abc")
bounded.now_ms = 20
bounded_collector:stop()
local bounded_sequences = events_of_type(bounded.events, "key_sequence")
assert(#bounded_sequences == 2)
assert(vim.deep_equal(bounded_sequences[1].keys, { "a", "b" }))
assert(vim.deep_equal(bounded_sequences[2].keys, { "c" }))

local special_collector, special = new_harness("aggregation-special")
special_collector:start()
special.now_ms = 10
special.callback("mapped-special-secret", "<C-X>a")
special.now_ms = 20
special_collector:stop()
local special_sequences = events_of_type(special.events, "key_sequence")
assert(#special_sequences == 1)
assert(vim.deep_equal(special_sequences[1].keys, { "<C-X>", "a" }))

local text_collector, text = new_harness("aggregation-text")
text_collector:start()
text.mode = "i"
for index, typed in ipairs({ "s", "e", "c" }) do
  text.now_ms = index * 10
  text.callback("mapped-" .. typed, typed)
end
text.now_ms = 35
text.callback("mapped-multiple-text", "xy")
text.mode = "c"
text.cmdtype = ":"
text.now_ms = 40
text.callback("mapped-command-secret", "command-secret")
text.cmdtype = "/"
text.now_ms = 50
text.callback("mapped-search-secret", "search-secret")
text.now_ms = 60
text_collector:stop()

local text_runs = events_of_type(text.events, "text_run")
assert(#text_runs == 1)
assert(text_runs[1].key_count == 5)
assert(text_runs[1].duration_ms == 25)
assert(text_runs[1].text == nil)

local transitions = events_of_type(text.events, "mode_transition")
assert(#transitions == 2)
assert(transitions[1].from == "insert" and transitions[1].to == "command")
assert(transitions[2].from == "command" and transitions[2].to == "search")

local text_json = vim.json.encode(text.events)
for _, secret in ipairs({ "mapped-s", "mapped-command-secret", "command-secret", "mapped-search-secret", "search-secret" }) do
  assert(string.find(text_json, secret, 1, true) == nil, "text-bearing input leaked into collector events")
end

local modal_collector, modal = new_harness("aggregation-modes")
modal_collector:start()
modal.mode = "v"
modal.now_ms = 10
modal.callback("x", "x")
modal.mode = "no"
modal.now_ms = 20
modal.callback("d", "d")
modal.now_ms = 30
modal_collector:stop()

local modal_sequences = events_of_type(modal.events, "key_sequence")
assert(#modal_sequences == 2)
assert(modal_sequences[1].mode == "visual")
assert(modal_sequences[2].mode == "operator_pending")

local excluded_collector, excluded = new_harness("aggregation-excluded")
excluded_collector:start()
excluded.now_ms = 10
excluded.callback("d", "d")
excluded.buffer = { buftype = "", filetype = "dotenv", name = "/work/.env" }
excluded.now_ms = 20
excluded.callback("mapped-sensitive-value", "typed-sensitive-value")
excluded.now_ms = 30
excluded_collector:stop()

local excluded_sequences = events_of_type(excluded.events, "key_sequence")
assert(#excluded_sequences == 1)
assert(vim.deep_equal(excluded_sequences[1].keys, { "d" }))
local excluded_json = vim.json.encode(excluded.events)
assert(string.find(excluded_json, "sensitive%-value") == nil, "excluded buffer input must never be persisted")

local select_collector, select_mode = new_harness("aggregation-select")
select_collector:start()
select_mode.mode = "s"
select_mode.now_ms = 10
select_mode.callback("mapped-select-secret", "select-secret")
select_mode.now_ms = 20
select_collector:stop()
assert(#events_of_type(select_mode.events, "key_sequence") == 0, "Select mode must not persist text as keys")
local select_runs = events_of_type(select_mode.events, "text_run")
assert(#select_runs == 1 and select_runs[1].key_count == vim.fn.strchars("select-secret"))
assert(string.find(vim.json.encode(select_mode.events), "select-secret", 1, true) == nil)

local retry = { callback = nil, now_ms = 0, write_state = "partial" }
local retry_directory = vim.fn.tempname()
local retry_fs = setmetatable({}, { __index = vim.uv })
retry_fs.fs_write = function(descriptor, data, offset)
  if retry.write_state == "partial" and string.find(data, '"event_type":"key_sequence"', 1, true) ~= nil then
    local partial = string.sub(data, 1, 8)
    local bytes_written, write_error = vim.uv.fs_write(descriptor, partial, offset)
    assert(bytes_written ~= nil, write_error)
    retry.write_state = "fail"
    return bytes_written
  end
  if retry.write_state == "fail" then
    retry.write_state = "recover"
    return nil, "injected aggregation write failure"
  end
  return vim.uv.fs_write(descriptor, data, offset)
end
local retry_store = storage.new({ directory = retry_directory, fs = retry_fs })
local retry_collector = collector.new({
  clock_ms = function()
    return retry.now_ms
  end,
  current_buffer = function()
    return { buftype = "", filetype = "lua", name = "src/retry.lua" }
  end,
  current_cmdtype = function()
    return ""
  end,
  current_mode = function()
    return "n"
  end,
  keytrans = function(key)
    return key
  end,
  new_session_id = function()
    return "aggregation-retry"
  end,
  open_session = function()
    return retry_store:open_session("aggregation-retry")
  end,
  register_on_key = function(callback)
    retry.callback = callback
    return function()
      retry.callback = nil
    end
  end,
})

retry_collector:start()
retry.now_ms = 10
retry.callback("mapped-retry-secret", "j")
retry.now_ms = 1011
retry.callback("mapped-trigger-secret", "k")
assert(retry_collector:status().state == "recording")
assert(retry_collector:status().pending_events == 1)
assert(string.find(retry_collector:status().last_error, "injected aggregation write failure", 1, true) ~= nil)
retry.now_ms = 1020
retry.callback("mapped-ignored-secret", "l")
retry.now_ms = 2021
retry.callback("mapped-second-boundary-secret", "m")
retry.now_ms = 2030
assert(retry_collector:stop())

local retry_lines =
  vim.fn.readfile(vim.fs.joinpath(retry_directory, "nvim-key-insights-aggregation-retry.jsonl"))
local retry_events = vim.tbl_map(vim.json.decode, retry_lines)
local retry_sequences = events_of_type(retry_events, "key_sequence")
assert(#retry_sequences == 1, "a retried aggregation event must be persisted exactly once")
assert(vim.deep_equal(retry_sequences[1].keys, { "j" }))
assert(#events_of_type(retry_events, "session_start") == 1)
assert(#events_of_type(retry_events, "session_end") == 1)
local retry_json = vim.json.encode(retry_events)
for _, secret in ipairs({ "retry-secret", "trigger-secret", "ignored-secret", "second-boundary-secret" }) do
  assert(string.find(retry_json, secret, 1, true) == nil)
end
vim.fn.delete(retry_directory, "rf")

local oversized_collector, oversized = new_harness(
  "aggregation-size-limit",
  config.resolve({ collection = { max_sequence_keys = 20000 } })
)
oversized_collector:start()
oversized.now_ms = 10
oversized.callback("mapped-oversized-secret", string.rep("j", 20000))
oversized.now_ms = 20
oversized_collector:stop()
local oversized_sequences = events_of_type(oversized.events, "key_sequence")
assert(#oversized_sequences > 1, "encoded event byte limit must split a large sequence")
local oversized_key_count = 0
for _, event in ipairs(oversized_sequences) do
  local encoded = schema.encode(event)
  assert(#encoded <= schema.MAX_EVENT_LINE_BYTES, "collector event must fit the analyzer line limit")
  oversized_key_count = oversized_key_count + #event.keys
end
assert(oversized_key_count == 20000, "byte-size splitting must preserve every typed key")

print("Lua aggregation contract: ok")
