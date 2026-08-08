local collector = require("key-insights.collector")
local config = require("key-insights.config")

local callback = nil
local buffer = vim.api.nvim_get_current_buf()
vim.keymap.set("n", "z9", ":echo 'PERFORMANCE_RHS_MUST_NOT_PERSIST'<CR>")

local instance = collector.new({
  auto_flush = false,
  clock_ms = function() return 1 end,
  current_buffer = function()
    return { id = buffer, buftype = "", filetype = "lua", name = "performance.lua" }
  end,
  current_cmdtype = function() return "" end,
  current_mode = function() return "n" end,
  new_session_id = function() return "performance-session" end,
  open_session = function()
    return {
      write = function() end,
      flush = function() end,
      finish = function() end,
      abort = function() end,
    }
  end,
  options = config.defaults(),
  register_on_key = function(handler)
    callback = handler
    return function() callback = nil end
  end,
})

assert(instance:start())
local iterations = 2000
local started = vim.uv.hrtime()
for _ = 1, iterations do
  assert(callback("mapped-output", "z9") == nil)
end
local elapsed_us = (vim.uv.hrtime() - started) / 1000
local average_us = elapsed_us / iterations

assert(average_us <= 500, string.format(
  "collector callback average %.2f us exceeds the 500 us regression budget",
  average_us
))

assert(instance:pause())
vim.keymap.del("n", "z9")
print(string.format("Lua callback performance: %.2f us average (budget 500 us)", average_us))
