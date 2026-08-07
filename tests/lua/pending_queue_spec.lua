local collector = require("key-insights.collector")
local config = require("key-insights.config")

local writes = 0
local buffer = vim.api.nvim_get_current_buf()
vim.keymap.set("n", "z8", "j")

local instance = collector.new({
  clock_ms = function() return 1 end,
  current_buffer = function()
    return { id = buffer, buftype = "", filetype = "lua", name = "pending-queue.lua" }
  end,
  current_cmdtype = function() return "" end,
  current_mode = function() return "n" end,
  new_session_id = function() return "pending-queue-session" end,
  open_session = function()
    return {
      write = function(_, lines) writes = writes + #lines end,
      flush = function() end,
      finish = function() end,
      abort = function() end,
    }
  end,
})

assert(instance:start())
vim.api.nvim_feedkeys(string.rep("z8", 20000), "xt", false)

local status = instance:status()
assert(status.pending_events <= 1024, "a synchronous input burst must not grow the pending event queue without bound")
assert(status.pending_bytes <= 4 * 1024 * 1024, "a synchronous input burst must not grow pending bytes without bound")
assert(status.last_error == "collector pending queue limit exceeded")
assert(writes == 1, "scheduled storage I/O must not run inside the synchronous feedkeys burst")

local accepted_before_stop = status.pending_events
assert(instance:stop())
assert(writes == accepted_before_stop + 2, "stop must finalize the accepted prefix without an overflow tail")
vim.keymap.del("n", "z8")

local callback = nil
local byte_clock = 0
local byte_limited = collector.new({
  auto_flush = true,
  clock_ms = function() return byte_clock end,
  current_buffer = function()
    return { id = buffer, buftype = "", filetype = "lua", name = "pending-bytes.lua" }
  end,
  current_cmdtype = function() return "" end,
  current_mode = function() return "n" end,
  keytrans = function(value) return value end,
  mapping_resolver = { prime = function() return false end, reset = function() end },
  new_session_id = function() return "pending-bytes-session" end,
  open_session = function()
    return {
      write = function() end,
      flush = function() end,
      finish = function() end,
      abort = function() end,
    }
  end,
  options = config.resolve({ collection = { max_sequence_keys = 20000 } }),
  register_on_key = function(handler)
    callback = handler
    return function() callback = nil end
  end,
  schedule = function() end,
})

assert(byte_limited:start())
local large_typed_input = string.rep("j", 20000)
for _ = 1, 70 do
  byte_clock = byte_clock + 1001
  assert(callback("mapped-RHS-must-not-escape", large_typed_input) == nil)
end
local byte_status = byte_limited:status()
assert(byte_status.pending_events < 1024, "the byte limit must stop this burst before the event limit")
assert(byte_status.pending_bytes <= 4 * 1024 * 1024)
assert(byte_status.last_error == "collector pending queue limit exceeded")
assert(string.find(vim.inspect(byte_status), "mapped-RHS", 1, true) == nil)
assert(byte_limited:stop())
assert(callback == nil)

print("Lua pending queue bound: ok")
