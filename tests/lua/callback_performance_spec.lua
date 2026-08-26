local collector = require("key-insights.collector")
local config = require("key-insights.config")

local MAPPING_ID = "mapping-v1:" .. string.rep("0", 64)
local CALLBACK_BUDGET_US = 500

local function new_harness()
  local state = {
    buffer = { id = vim.api.nvim_get_current_buf(), buftype = "", filetype = "lua", name = "performance.lua" },
    callback = nil,
    callback_active = false,
    keytrans_calls = 0,
    mode = "n",
    now_ms = 0,
    resolve_calls = 0,
    scheduled = {},
    schedule_calls = 0,
    writes = 0,
    writes_in_callback = 0,
  }
  local resolver = {
    boundary = function() end,
    prime = function() return true end,
    reset = function() end,
    resolve = function(_, _, tokens)
      state.resolve_calls = state.resolve_calls + 1
      if vim.deep_equal(tokens, { "z", "9" }) then
        return { mapping_id = MAPPING_ID, mode = "normal", scope = "global" }
      end
      return nil
    end,
  }
  local instance = collector.new({
    auto_flush = true,
    clock_ms = function() return state.now_ms end,
    current_buffer = function() return state.buffer end,
    current_cmdtype = function() return "" end,
    current_mode = function() return state.mode end,
    keytrans = function(value)
      state.keytrans_calls = state.keytrans_calls + 1
      return value
    end,
    mapping_resolver = resolver,
    new_session_id = function() return "performance-session" end,
    open_session = function()
      return {
        write = function(_, lines)
          state.writes = state.writes + #lines
          if state.callback_active then
            state.writes_in_callback = state.writes_in_callback + #lines
          end
        end,
        flush = function() end,
        finish = function() end,
        abort = function() end,
      }
    end,
    options = config.defaults(),
    register_on_key = function(handler)
      state.callback = handler
      return function() state.callback = nil end
    end,
    schedule = function(fn)
      state.schedule_calls = state.schedule_calls + 1
      table.insert(state.scheduled, fn)
    end,
  })

  function state:invoke(mapped, typed)
    self.callback_active = true
    local result = self.callback(mapped, typed)
    self.callback_active = false
    assert(result == nil, "collector callbacks must never consume input")
  end

  function state:drain()
    local pending = self.scheduled
    self.scheduled = {}
    for _, fn in ipairs(pending) do
      fn()
    end
  end

  assert(instance:start())
  return instance, state
end

local function median(values)
  table.sort(values)
  return values[math.floor((#values + 1) / 2)]
end

local function measure_path(label, configure, mapped, typed)
  local instance, state = new_harness()
  configure(state)
  local batch_size = 128
  local warmup_batches = 2
  local measured_batches = 7
  local samples = {}

  for batch = 1, warmup_batches + measured_batches do
    local started = vim.uv.hrtime()
    for _ = 1, batch_size do
      state:invoke(mapped, typed)
    end
    local average_us = ((vim.uv.hrtime() - started) / 1000) / batch_size
    if batch > warmup_batches then
      table.insert(samples, average_us)
    end
    assert(instance:status().last_error == nil, label .. " timing path must remain active")
    assert(instance:status().pending_events <= 1024)
    assert(instance:status().pending_bytes <= 4 * 1024 * 1024)
    state:drain()
    instance:flush()
  end

  local median_us = median(samples)
  assert(median_us <= CALLBACK_BUDGET_US, string.format(
    "%s callback median %.2f us exceeds the %d us regression budget",
    label,
    median_us,
    CALLBACK_BUDGET_US
  ))
  assert(state.writes_in_callback == 0, label .. " callback must not perform storage I/O")
  assert(instance:pause())
  return median_us
end

-- Deterministic operation-count contracts are the primary callback budget.
local excluded_instance, excluded = new_harness()
excluded.buffer = { id = 1, buftype = "terminal", filetype = "", name = "" }
excluded:invoke("private-mapped-output", "private-typed-input")
assert(excluded.keytrans_calls == 0)
assert(excluded.resolve_calls == 0)
assert(excluded.schedule_calls == 0)
assert(excluded.writes_in_callback == 0)
assert(excluded_instance:pause())

local ordinary_instance, ordinary = new_harness()
ordinary:invoke("h", "h")
assert(ordinary.keytrans_calls == 1)
assert(ordinary.resolve_calls == 1)
assert(ordinary.schedule_calls == 0, "an ordinary key must stay aggregated without scheduling I/O")
assert(ordinary.writes_in_callback == 0)
assert(ordinary_instance:pause())

local mapped_instance, mapped = new_harness()
for _ = 1, 32 do
  mapped:invoke("mapped-output-must-not-persist", "z9")
end
assert(mapped.keytrans_calls == 32)
assert(mapped.resolve_calls == 32)
assert(mapped.schedule_calls == 1, "a callback burst must coalesce scheduled flushes")
assert(#mapped.scheduled == 1)
assert(mapped.writes_in_callback == 0)
assert(mapped_instance:status().pending_events == 32)
mapped:drain()
assert(mapped_instance:status().pending_events == 0)
assert(mapped_instance:pause())

local boundary_instance, boundary = new_harness()
boundary.now_ms = 1
boundary:invoke("h", "h")
boundary.now_ms = 1002
boundary:invoke("j", "j")
assert(boundary.schedule_calls == 1, "an idle sequence boundary must schedule one deferred write")
assert(boundary.writes_in_callback == 0)
boundary:drain()
assert(boundary_instance:pause())

local insert_instance, insert = new_harness()
insert.mode = "i"
insert:invoke("insert-mapped-secret", "insert-typed-secret")
assert(insert.keytrans_calls == 1)
assert(insert.resolve_calls == 0)
assert(insert.schedule_calls == 0, "an Insert text run must remain aggregated in memory")
assert(insert.writes_in_callback == 0)
assert(insert_instance:pause())

local timings = {
  excluded = measure_path("excluded", function(state)
    state.buffer = { id = 1, buftype = "terminal", filetype = "", name = "" }
  end, "private-mapped-output", "private-typed-input"),
  ordinary = measure_path("ordinary", function() end, "h", "h"),
  mapped = measure_path("mapped", function() end, "mapped-output-must-not-persist", "z9"),
  insert = measure_path("insert", function(state) state.mode = "i" end, "insert-mapped-secret", "x"),
}

print(string.format(
  "Lua callback telemetry (median us, budget %d): excluded %.2f, ordinary %.2f, mapped %.2f, insert %.2f",
  CALLBACK_BUDGET_US,
  timings.excluded,
  timings.ordinary,
  timings.mapped,
  timings.insert
))
