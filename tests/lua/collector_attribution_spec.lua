local collector = require("key-insights.collector")

local function memory_session(events, on_write)
  return {
    write = function(_, lines)
      if on_write ~= nil then
        on_write()
      end
      for _, line in ipairs(lines) do
        table.insert(events, vim.json.decode(line))
      end
    end,
    flush = function() end,
    finish = function() end,
    abort = function() end,
  }
end

local function count(events, event_type)
  local result = 0
  for _, event in ipairs(events) do
    if event.event_type == event_type then
      result = result + 1
    end
  end
  return result
end

local function fixture(overrides)
  local events = {}
  local callback = nil
  local in_callback = false
  local writes_in_callback = 0
  local mode = "n"
  local buffer = { id = 7, buftype = "", filetype = "lua", name = "" }
  local resolver = {
    prime = function(_, resolved_buffer)
      assert(resolved_buffer == 7)
      return true
    end,
    reset = function() end,
    resolve = function(_, resolved_mode, typed_keys, ...)
      assert(select("#", ...) == 0, "mapped input must never cross the resolver boundary")
      if resolved_mode == "normal" and vim.deep_equal(typed_keys, { "z", "q" }) then
        return {
          mapping_id = "mapping-v1:" .. string.rep("a", 64),
          mode = "normal",
          scope = "global",
        }
      end
    end,
  }
  local spec = vim.tbl_extend("force", {
    clock_ms = function()
      return 10
    end,
    current_buffer = function()
      return buffer
    end,
    current_cmdtype = function()
      return ""
    end,
    current_mode = function()
      return mode
    end,
    keytrans = function(value)
      return value
    end,
    mapping_resolver = resolver,
    new_session_id = function()
      return "attribution-session"
    end,
    open_session = function()
      return memory_session(events, function()
        if in_callback then
          writes_in_callback = writes_in_callback + 1
        end
      end)
    end,
    register_on_key = function(value)
      callback = value
      return function()
        callback = nil
      end
    end,
  }, overrides or {})
  return collector.new(spec), events, resolver, {
    callback = function(mapped, typed)
      in_callback = true
      local result = { pcall(callback, mapped, typed) }
      in_callback = false
      assert(result[1], result[2])
      return unpack(result, 2)
    end,
    set_buffer = function(value)
      buffer = value
    end,
    set_mode = function(value)
      mode = value
    end,
    writes_in_callback = function()
      return writes_in_callback
    end,
  }
end

local instance, events, _, controls = fixture()
assert(instance:start() == true)
local mapped_secret = "mapped-/private/credential-SECRET"
assert(controls.callback(mapped_secret, "zq") == nil, "collector callbacks must not consume input")
assert(controls.writes_in_callback() == 0, "mapping attribution must not perform callback-path storage I/O")
instance:flush()
assert(count(events, "mapping_use") == 1, "one confirmed callback must emit exactly one mapping_use")
assert(count(events, "key_sequence") == 1, "attributed keys must remain ordinary sequence evidence")
for _, event in ipairs(events) do
  local encoded = vim.json.encode(event)
  assert(string.find(encoded, mapped_secret, 1, true) == nil)
  assert(string.find(encoded, "/private/credential", 1, true) == nil)
end
assert(string.find(vim.inspect(instance:status()), "/private/credential", 1, true) == nil)
local mapping_event = nil
for _, event in ipairs(events) do
  if event.event_type == "mapping_use" then
    mapping_event = event
  end
end
assert(mapping_event.mapping_id == "mapping-v1:" .. string.rep("a", 64))
assert(mapping_event.mode == "normal")
assert(vim.deep_equal(mapping_event.typed_keys, { "z", "q" }))
instance:stop()

local gated, gated_events, gated_resolver, gated_controls = fixture()
local resolve_calls = 0
gated_resolver.resolve = function()
  resolve_calls = resolve_calls + 1
  return { mapping_id = "mapping-v1:" .. string.rep("b", 64), mode = "normal", scope = "global" }
end
gated:start()
gated_controls.set_mode("i")
gated_controls.callback("private inserted text", "x")
gated_controls.set_mode("c")
gated_controls.callback("private command", "x")
gated_controls.set_mode("n")
gated_controls.callback("", "zq")
gated_controls.set_buffer({ id = 8, buftype = "terminal", filetype = "", name = "" })
gated_controls.callback("private terminal output", "zq")
gated:flush()
assert(resolve_calls == 0, "text, unsupported, and excluded input must not reach attribution")
assert(count(gated_events, "mapping_use") == 0)
gated:stop()

local lifecycle, _, lifecycle_resolver, lifecycle_controls = fixture()
local primes = 0
local resets = 0
lifecycle_resolver.prime = function()
  primes = primes + 1
  return true
end
lifecycle_resolver.reset = function()
  resets = resets + 1
end
lifecycle:start()
assert(primes == 1, "start must prime attribution outside the callback")
lifecycle_controls.callback("j", "zq")
lifecycle:flush()
assert(resets >= 1, "flush must discard attribution state")
lifecycle:pause()
local resets_after_pause = resets
local primes_before_resume = primes
assert(lifecycle:start() == true)
assert(primes > primes_before_resume, "resume must establish a fresh attribution baseline")
lifecycle_controls.set_mode("v")
lifecycle_controls.callback("l", "zq")
assert(resets > resets_after_pause, "mode transitions must not retain attribution state")
lifecycle:stop()

local resilient, resilient_events, resilient_resolver, resilient_controls = fixture()
local attempts = 0
resilient_resolver.resolve = function()
  attempts = attempts + 1
  if attempts == 1 then
    error("resolver-private-failure")
  end
  return nil
end
resilient:start()
resilient_controls.callback("private mapped value", "zq")
resilient_controls.callback("x", "x")
resilient:flush()
assert(resilient:status().last_error == nil, "attribution failure must not poison collection")
assert(count(resilient_events, "mapping_use") == 0)
assert(count(resilient_events, "key_sequence") == 1)
resilient:stop()

local unavailable, unavailable_events, unavailable_resolver, unavailable_controls = fixture()
unavailable_resolver.prime = function()
  return nil
end
unavailable:start()
unavailable_controls.callback("private mapped value", "zq")
unavailable:flush()
assert(unavailable:status().last_error == nil, "an unavailable baseline must not poison collection")
assert(count(unavailable_events, "mapping_use") == 0)
assert(count(unavailable_events, "key_sequence") == 1)
unavailable:stop()

print("Lua collector attribution: ok")
