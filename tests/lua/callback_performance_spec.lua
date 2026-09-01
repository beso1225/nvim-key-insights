local collector = require("key-insights.collector")
local config = require("key-insights.config")

local MAPPING_ID = "mapping-v1:" .. string.rep("0", 64)
local CALLBACK_BUDGET_US = 500

local function new_harness(use_real_resolver)
  local state = {
    buffer = { id = vim.api.nvim_get_current_buf(), buftype = "", filetype = "lua", name = "performance.lua" },
    boundary_calls = 0,
    callback = nil,
    callback_active = false,
    clock_calls = 0,
    cmdtype_calls = 0,
    current_buffer_calls = 0,
    current_mode_calls = 0,
    events = {},
    keytrans_calls = 0,
    mode = "n",
    now_ms = 0,
    resolve_calls = 0,
    scheduled = {},
    schedule_calls = 0,
    storage_calls_in_callback = 0,
    writes = 0,
  }
  local resolver = {
    boundary = function() state.boundary_calls = state.boundary_calls + 1 end,
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
  local dependencies = {
    auto_flush = true,
    clock_ms = function()
      state.clock_calls = state.clock_calls + 1
      return state.now_ms
    end,
    current_buffer = function()
      state.current_buffer_calls = state.current_buffer_calls + 1
      return state.buffer
    end,
    current_cmdtype = function()
      state.cmdtype_calls = state.cmdtype_calls + 1
      return ""
    end,
    current_mode = function()
      state.current_mode_calls = state.current_mode_calls + 1
      return state.mode
    end,
    keytrans = function(value)
      state.keytrans_calls = state.keytrans_calls + 1
      return value
    end,
    new_session_id = function() return "performance-session" end,
    open_session = function()
      local function storage_call()
        if state.callback_active then
          state.storage_calls_in_callback = state.storage_calls_in_callback + 1
        end
      end
      return {
        write = function(_, lines)
          storage_call()
          state.writes = state.writes + #lines
          for _, line in ipairs(lines) do
            table.insert(state.events, vim.json.decode(line))
          end
        end,
        flush = storage_call,
        finish = storage_call,
        abort = storage_call,
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
  }
  if not use_real_resolver then
    dependencies.mapping_resolver = resolver
  end
  local instance = collector.new(dependencies)

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

  function state:reset_counts()
    self.boundary_calls = 0
    self.clock_calls = 0
    self.cmdtype_calls = 0
    self.current_buffer_calls = 0
    self.current_mode_calls = 0
    self.events = {}
    self.keytrans_calls = 0
    self.resolve_calls = 0
    self.schedule_calls = 0
    self.storage_calls_in_callback = 0
    self.writes = 0
  end

  assert(instance:start())
  state:reset_counts()
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

local function median(values)
  table.sort(values)
  return values[math.floor((#values + 1) / 2)]
end

local function measure_path(label, configure, mapped, typed, use_real_resolver)
  local instance, state = new_harness(use_real_resolver)
  configure(state)
  local batch_size = 128
  local warmup_batches = 2
  local measured_batches = 7
  local samples = {}
  local total_callbacks = batch_size * (warmup_batches + measured_batches)

  for batch = 1, warmup_batches + measured_batches do
    local before = {
      clock = state.clock_calls,
      cmdtype = state.cmdtype_calls,
      current_buffer = state.current_buffer_calls,
      current_mode = state.current_mode_calls,
      keytrans = state.keytrans_calls,
      resolve = state.resolve_calls,
    }
    local started = vim.uv.hrtime()
    for _ = 1, batch_size do
      state:invoke(mapped, typed)
    end
    local average_us = ((vim.uv.hrtime() - started) / 1000) / batch_size
    if batch > warmup_batches then
      table.insert(samples, average_us)
    end
    assert(instance:status().state == "recording", label .. " timing path must remain recording")
    assert(instance:status().last_error == nil, label .. " timing path must remain active")
    assert(instance:status().pending_events <= 1024)
    assert(instance:status().pending_bytes <= 4 * 1024 * 1024)
    assert(state.clock_calls - before.clock == batch_size)
    assert(state.current_buffer_calls - before.current_buffer == batch_size)
    if label == "excluded" then
      assert(state.current_mode_calls - before.current_mode == 0)
      assert(state.cmdtype_calls - before.cmdtype == 0)
      assert(state.keytrans_calls - before.keytrans == 0)
      assert(state.resolve_calls - before.resolve == 0)
    else
      assert(state.current_mode_calls - before.current_mode == batch_size)
      assert(state.cmdtype_calls - before.cmdtype == batch_size)
      assert(state.keytrans_calls - before.keytrans == batch_size)
      if not use_real_resolver then
        local expected_resolves = label == "insert" and 0 or batch_size
        assert(state.resolve_calls - before.resolve == expected_resolves)
      end
    end
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
  if label == "excluded" then
    assert(#state.events == 0)
  end
  if label == "mapped" then
    assert(#events_of_type(state.events, "mapping_use") == total_callbacks)
  elseif label == "ordinary" then
    local key_count = 0
    for _, event in ipairs(events_of_type(state.events, "key_sequence")) do
      key_count = key_count + #event.keys
    end
    assert(key_count == total_callbacks)
  elseif label == "insert" then
    local key_count = 0
    for _, event in ipairs(events_of_type(state.events, "text_run")) do
      key_count = key_count + event.key_count
    end
    assert(key_count == total_callbacks)
  end
  assert(state.storage_calls_in_callback == 0, label .. " callback must not perform storage I/O")
  assert(instance:pause())
  return { max = math.max(unpack(samples)), median = median_us, min = math.min(unpack(samples)) }
end

-- Deterministic operation-count contracts are the primary callback budget.
local excluded_instance, excluded = new_harness()
excluded.buffer = { id = 1, buftype = "terminal", filetype = "", name = "" }
excluded:invoke("private-mapped-output", "private-typed-input")
assert(excluded.clock_calls == 1 and excluded.current_buffer_calls == 1)
assert(excluded.current_mode_calls == 0 and excluded.cmdtype_calls == 0)
assert(excluded.keytrans_calls == 0)
assert(excluded.resolve_calls == 0)
assert(excluded.schedule_calls == 0)
assert(excluded.boundary_calls == 1)
assert(excluded.storage_calls_in_callback == 0)
assert(excluded_instance:pause())
assert(#excluded.events == 0)

local ordinary_instance, ordinary = new_harness()
ordinary:invoke("h", "h")
assert(ordinary.clock_calls == 1 and ordinary.current_buffer_calls == 1)
assert(ordinary.current_mode_calls == 1 and ordinary.cmdtype_calls == 1)
assert(ordinary.keytrans_calls == 1)
assert(ordinary.resolve_calls == 1)
assert(ordinary.schedule_calls == 0, "an ordinary key must stay aggregated without scheduling I/O")
assert(ordinary.boundary_calls == 1)
assert(ordinary.storage_calls_in_callback == 0)
assert(ordinary_instance:pause())
local ordinary_sequences = events_of_type(ordinary.events, "key_sequence")
assert(#ordinary_sequences == 1 and vim.deep_equal(ordinary_sequences[1].keys, { "h" }))

local mapped_instance, mapped = new_harness()
for _ = 1, 32 do
  mapped:invoke("mapped-output-must-not-persist", "z9")
end
assert(mapped.keytrans_calls == 32)
assert(mapped.resolve_calls == 32)
assert(mapped.schedule_calls == 1, "a callback burst must coalesce scheduled flushes")
assert(#mapped.scheduled == 1)
assert(mapped.clock_calls == 32 and mapped.current_buffer_calls == 32)
assert(mapped.current_mode_calls == 32 and mapped.cmdtype_calls == 32)
assert(mapped.boundary_calls == 1)
assert(mapped.storage_calls_in_callback == 0)
assert(mapped_instance:status().pending_events == 32)
mapped:drain()
assert(mapped_instance:status().pending_events == 0)
assert(mapped_instance:pause())
assert(#events_of_type(mapped.events, "mapping_use") == 32)
assert(string.find(vim.json.encode(mapped.events), "mapped-output", 1, true) == nil)

local boundary_instance, boundary = new_harness()
boundary.now_ms = 1
boundary:invoke("h", "h")
boundary.now_ms = 1002
boundary:invoke("j", "j")
assert(boundary.clock_calls == 2 and boundary.current_buffer_calls == 2)
assert(boundary.current_mode_calls == 2 and boundary.cmdtype_calls == 2)
assert(boundary.keytrans_calls == 2 and boundary.resolve_calls == 2)
assert(boundary.boundary_calls == 1)
assert(boundary.schedule_calls == 1, "an idle sequence boundary must schedule one deferred write")
assert(boundary_instance:status().pending_events == 1)
assert(boundary.storage_calls_in_callback == 0)
boundary:drain()
assert(boundary_instance:status().pending_events == 0)
local first_boundary = events_of_type(boundary.events, "key_sequence")
assert(#first_boundary == 1 and vim.deep_equal(first_boundary[1].keys, { "h" }))
assert(boundary_instance:pause())
local boundary_sequences = events_of_type(boundary.events, "key_sequence")
assert(#boundary_sequences == 2 and vim.deep_equal(boundary_sequences[2].keys, { "j" }))

local insert_instance, insert = new_harness()
insert.mode = "i"
insert.now_ms = 10
insert:invoke("insert-mapped-secret-a", "a")
insert.now_ms = 20
insert:invoke("insert-mapped-secret-b", "界")
insert.now_ms = 30
insert:invoke("insert-mapped-secret-c", "<C-X>")
assert(insert.clock_calls == 3 and insert.current_buffer_calls == 3)
assert(insert.current_mode_calls == 3 and insert.cmdtype_calls == 3)
assert(insert.keytrans_calls == 3)
assert(insert.resolve_calls == 0)
assert(insert.schedule_calls == 0, "an Insert text run must remain aggregated in memory")
assert(insert.boundary_calls == 1)
assert(insert.storage_calls_in_callback == 0)
assert(insert_instance:pause())
local text_runs = events_of_type(insert.events, "text_run")
assert(#text_runs == 1 and text_runs[1].key_count == 3 and text_runs[1].duration_ms == 20)
assert(string.find(vim.json.encode(insert.events), "insert-mapped-secret", 1, true) == nil)

local function callback_failure_harness(overrides)
  local callback = nil
  local storage_calls = 0
  local function storage_call()
    storage_calls = storage_calls + 1
  end
  local dependencies = {
    auto_flush = true,
    clock_ms = function() return 0 end,
    current_buffer = function()
      return { id = vim.api.nvim_get_current_buf(), buftype = "", filetype = "lua", name = "failure.lua" }
    end,
    current_cmdtype = function() return "" end,
    current_mode = function() return "n" end,
    keytrans = function(value) return value end,
    mapping_resolver = {
      boundary = function() end,
      prime = function() return true end,
      reset = function() end,
      resolve = function()
        return { mapping_id = MAPPING_ID, mode = "normal", scope = "global" }
      end,
    },
    new_session_id = function() return "callback-failure-session" end,
    open_session = function()
      return { write = storage_call, flush = storage_call, finish = storage_call, abort = storage_call }
    end,
    options = config.defaults(),
    register_on_key = function(handler)
      callback = handler
      return function() callback = nil end
    end,
    schedule = function() end,
  }
  for key, value in pairs(overrides or {}) do
    dependencies[key] = value
  end
  local instance = collector.new(dependencies)
  assert(instance:start())
  return instance, function(mapped, typed)
    assert(callback(mapped, typed) == nil)
  end, function() return storage_calls end
end

local callback_error, invoke_callback_error = callback_failure_harness({
  keytrans = function(value) error("PRIVATE_CALLBACK_INPUT:" .. value) end,
})
invoke_callback_error("private-mapped", "private-typed")
assert(callback_error:status().last_error == "collector callback failed")
assert(string.find(vim.inspect(callback_error:status()), "PRIVATE_CALLBACK_INPUT", 1, true) == nil)

local synchronous_schedule, invoke_synchronous_schedule, synchronous_storage_calls = callback_failure_harness({
  schedule = function(fn) fn() end,
})
local synchronous_storage_baseline = synchronous_storage_calls()
invoke_synchronous_schedule("mapped-output", "z9")
assert(synchronous_schedule:status().last_error == "collector scheduler contract violated")
assert(synchronous_storage_calls() == synchronous_storage_baseline)
assert(synchronous_schedule:status().pending_events == 1)

local throwing_schedule, invoke_throwing_schedule = callback_failure_harness({
  schedule = function() error("PRIVATE_SCHEDULER_DETAIL") end,
})
invoke_throwing_schedule("mapped-output", "z9")
assert(throwing_schedule:status().last_error == "collector scheduler contract violated")
assert(string.find(vim.inspect(throwing_schedule:status()), "PRIVATE_SCHEDULER_DETAIL", 1, true) == nil)

local function dirty_resolver()
  return {
    boundary = function() end,
    is_dirty = function() return true end,
    prime = function() return true end,
    reset = function() end,
    resolve = function() return nil end,
  }
end

local synchronous_reprime, invoke_synchronous_reprime, synchronous_reprime_storage = callback_failure_harness({
  mapping_resolver = dirty_resolver(),
  schedule = function(fn) fn() end,
})
local synchronous_reprime_storage_baseline = synchronous_reprime_storage()
invoke_synchronous_reprime("mapped-output", "z9")
assert(synchronous_reprime:status().last_error == "collector scheduler contract violated")
assert(synchronous_reprime:status().pending_events == 0)
assert(synchronous_reprime_storage() == synchronous_reprime_storage_baseline)

local throwing_reprime, invoke_throwing_reprime = callback_failure_harness({
  mapping_resolver = dirty_resolver(),
  schedule = function() error("PRIVATE_REPRIME_SCHEDULER_DETAIL") end,
})
invoke_throwing_reprime("mapped-output", "z9")
assert(throwing_reprime:status().last_error == "collector scheduler contract violated")
assert(throwing_reprime:status().pending_events == 0)
assert(string.find(vim.inspect(throwing_reprime:status()), "PRIVATE_REPRIME_SCHEDULER_DETAIL", 1, true) == nil)

vim.keymap.set("n", "z9", ":echo 'PERFORMANCE_RHS_MUST_NOT_PERSIST'<CR>")
local timings = {
  excluded = measure_path("excluded", function(state)
    state.buffer = { id = 1, buftype = "terminal", filetype = "", name = "" }
  end, "private-mapped-output", "private-typed-input"),
  ordinary = measure_path("ordinary", function() end, "h", "h"),
  mapped = measure_path("mapped", function() end, "mapped-output-must-not-persist", "z9", true),
  insert = measure_path("insert", function(state) state.mode = "i" end, "insert-mapped-secret", "x"),
}
vim.keymap.del("n", "z9")

local uname = vim.uv.os_uname()
local version = vim.version()
print(string.format(
  "Lua callback telemetry (%s/%s, Neovim %d.%d.%d, 7x128 measured after 2 warmups, budget %d us median): "
    .. "excluded %.2f [%.2f, %.2f], ordinary %.2f [%.2f, %.2f], "
    .. "mapped %.2f [%.2f, %.2f], insert %.2f [%.2f, %.2f]",
  uname.sysname,
  uname.machine,
  version.major,
  version.minor,
  version.patch,
  CALLBACK_BUDGET_US,
  timings.excluded.median,
  timings.excluded.min,
  timings.excluded.max,
  timings.ordinary.median,
  timings.ordinary.min,
  timings.ordinary.max,
  timings.mapped.median,
  timings.mapped.min,
  timings.mapped.max,
  timings.insert.median,
  timings.insert.min,
  timings.insert.max
))
