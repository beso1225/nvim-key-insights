local collector = require("key-insights.collector")

local now_ms = 0
local written = {}
local active_callback = nil
local unregister_count = 0

local instance = collector.new({
  clock_ms = function()
    return now_ms
  end,
  new_session_id = function()
    return "session-one"
  end,
  write = function(lines)
    for _, line in ipairs(lines) do
      table.insert(written, vim.json.decode(line))
    end
  end,
  register_on_key = function(callback)
    active_callback = callback
    return function()
      active_callback = nil
      unregister_count = unregister_count + 1
    end
  end,
  current_buffer = function()
    return { buftype = "", filetype = "lua", name = "src/init.lua" }
  end,
})

assert(instance:status().state == "stopped")
assert(instance:start() == true)
assert(instance:status().state == "recording")
assert(instance:status().session_id == "session-one")
assert(active_callback ~= nil, "start must register vim.on_key collection")
assert(#written == 1 and written[1].event_type == "session_start")

assert(active_callback("mapped", "typed") == nil, "collector callback must never consume input")
assert(#written == 1, "lifecycle callback must not persist raw input")

now_ms = 10
assert(instance:pause() == true)
assert(instance:status().state == "paused")
assert(active_callback == nil, "pause must detach collection")
assert(unregister_count == 1)

now_ms = 25
assert(instance:start() == true, "start must resume a paused session")
assert(instance:status().session_id == "session-one", "resume must preserve the session boundary")
assert(#written == 1, "resume must not write a second session_start")

now_ms = 40
assert(instance:stop() == true)
assert(instance:status().state == "stopped")
assert(active_callback == nil)
assert(unregister_count == 2)
assert(#written == 2 and written[2].event_type == "session_end")
assert(written[2].elapsed_ms == 40)
assert(instance:stop() == false, "stop must be idempotent")

local pending_writes = 0
local buffered = collector.new({
  new_session_id = function()
    return "session-two"
  end,
  write = function(lines)
    pending_writes = pending_writes + #lines
  end,
  register_on_key = function()
    return function() end
  end,
  auto_flush = false,
})

buffered:start()
assert(pending_writes == 0)
assert(buffered:flush() == 1)
assert(pending_writes == 1)
buffered:stop()
assert(pending_writes == 2)
assert(buffered:flush() == 0)

local pause_writes = 0
local pause_buffered = collector.new({
  new_session_id = function()
    return "session-pause"
  end,
  write = function(lines)
    pause_writes = pause_writes + #lines
  end,
  register_on_key = function()
    return function() end
  end,
  auto_flush = false,
})
pause_buffered:start()
pause_buffered:pause()
assert(pause_writes == 1, "pause must flush buffered events")

local excluded_callback = nil
local excluded_writes = {}
local excluded = collector.new({
  new_session_id = function()
    return "session-three"
  end,
  write = function(lines)
    vim.list_extend(excluded_writes, lines)
  end,
  register_on_key = function(callback)
    excluded_callback = callback
    return function() end
  end,
  current_buffer = function()
    return { buftype = "", filetype = "dotenv", name = "/work/.env" }
  end,
})

excluded:start()
local before = #excluded_writes
assert(excluded_callback("mapped-secret", "typed-secret") == nil)
assert(#excluded_writes == before, "sensitive buffers must not emit input events")
excluded:stop()

local storage = require("key-insights.storage")
local temporary_directory = vim.fn.tempname()
local log_path = temporary_directory .. "/events.jsonl"
local writer = storage.new({ path = log_path })
writer:write({ "first\n", "second\n" })
assert(vim.deep_equal(vim.fn.readfile(log_path), { "first", "second" }))
local permissions = vim.uv.fs_stat(log_path).mode % 512
assert(permissions == 384, "collector logs must be readable only by their owner")
vim.fn.delete(temporary_directory, "rf")

dofile("plugin/key-insights.lua")
local commands = vim.api.nvim_get_commands({})
for _, name in ipairs({
  "KeyInsightsStart",
  "KeyInsightsPause",
  "KeyInsightsStop",
  "KeyInsightsStatus",
}) do
  assert(commands[name] ~= nil, name .. " must be registered")
end

local command_log_directory = vim.fn.tempname()
local command_log_path = command_log_directory .. "/events.jsonl"
local api = require("key-insights")
api.setup({ storage = { path = command_log_path } })
vim.cmd.KeyInsightsStart()
vim.cmd.KeyInsightsStop()
local command_events = vim.fn.readfile(command_log_path)
assert(#command_events == 2)
local command_start = vim.json.decode(command_events[1])
local command_end = vim.json.decode(command_events[2])
assert(command_start.event_type == "session_start")
assert(command_end.event_type == "session_end")
assert(command_start.session_id == command_end.session_id)
vim.fn.delete(command_log_directory, "rf")

print("Lua collector contract: ok")
