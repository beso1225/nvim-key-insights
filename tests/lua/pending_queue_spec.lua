local collector = require("key-insights.collector")

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

assert(instance:stop())
vim.keymap.del("n", "z8")

local callback = nil
local byte_limited = collector.new({
  clock_ms = function() return 1 end,
  current_buffer = function()
    return { id = buffer, buftype = "", filetype = "lua", name = "pending-bytes.lua" }
  end,
  current_cmdtype = function() return "" end,
  current_mode = function() return "n" end,
  keytrans = function(value) return value end,
  mapping_resolver = {
    prime = function() return true end,
    reset = function() end,
    resolve = function()
      return {
        mapping_id = "mapping-v1:" .. string.rep("a", 64),
        mode = "normal",
        scope = "global",
      }
    end,
  },
  new_session_id = function() return "pending-bytes-session" end,
  open_session = function()
    return {
      write = function() end,
      flush = function() end,
      finish = function() end,
      abort = function() end,
    }
  end,
  pending_byte_limit = 128,
  register_on_key = function(handler)
    callback = handler
    return function() callback = nil end
  end,
  schedule = function() end,
})

assert(byte_limited:start())
assert(callback("mapped-RHS-must-not-escape", "zq") == nil)
local byte_status = byte_limited:status()
assert(byte_status.pending_events == 0 and byte_status.pending_bytes == 0)
assert(byte_status.last_error == "collector pending queue limit exceeded")
assert(string.find(vim.inspect(byte_status), "mapped-RHS", 1, true) == nil)
assert(byte_limited:stop())
assert(callback == nil)

print("Lua pending queue bound: ok")
