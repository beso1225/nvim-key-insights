local collector = require("key-insights.collector")
local schema = require("key-insights.schema")

local function memory_session(on_write)
  return {
    write = function(_, lines)
      on_write(lines)
    end,
    flush = function() end,
    finish = function() end,
    abort = function() end,
  }
end

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
  open_session = function()
    return memory_session(function(lines)
      for _, line in ipairs(lines) do
        table.insert(written, vim.json.decode(line))
      end
    end)
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
assert(active_callback("mapped", "t") == nil, "collector callback must never consume input")
assert(#written == 1, "input aggregation must remain buffered until a sequence boundary")

now_ms = 10
assert(instance:pause() == true)
assert(instance:status().state == "paused")
assert(active_callback == nil, "pause must detach collection")
assert(unregister_count == 1)
assert(written[2].event_type == "key_sequence")
assert(vim.deep_equal(written[2].keys, { "t" }))

now_ms = 25
assert(instance:start() == true, "start must resume a paused session")
assert(instance:status().session_id == "session-one", "resume must preserve the session boundary")
local session_starts = 0
for _, event in ipairs(written) do
  if event.event_type == "session_start" then
    session_starts = session_starts + 1
  end
end
assert(session_starts == 1, "resume must not write a second session_start")

now_ms = 40
assert(instance:stop() == true)
assert(instance:status().state == "stopped")
assert(active_callback == nil)
assert(unregister_count == 2)
assert(#written == 3 and written[3].event_type == "session_end")
assert(written[3].elapsed_ms == 40)
assert(instance:stop() == false, "stop must be idempotent")

local pending_writes = 0
local buffered = collector.new({
  new_session_id = function()
    return "session-two"
  end,
  open_session = function()
    return memory_session(function(lines)
      pending_writes = pending_writes + #lines
    end)
  end,
  register_on_key = function()
    return function() end
  end,
  auto_flush = false,
})

buffered:start()
assert(pending_writes == 1)
assert(buffered:flush() == 0)
buffered:stop()
assert(pending_writes == 2)
assert(buffered:flush() == 0)

local pause_writes = 0
local pause_buffered = collector.new({
  new_session_id = function()
    return "session-pause"
  end,
  open_session = function()
    return memory_session(function(lines)
      pause_writes = pause_writes + #lines
    end)
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
  open_session = function()
    return memory_session(function(lines)
      vim.list_extend(excluded_writes, lines)
    end)
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

local start_aborted = false
local start_registered = false
local failing_start = collector.new({
  new_session_id = function()
    return "session-start-failure"
  end,
  open_session = function()
    return {
      write = function()
        error("disk full")
      end,
      abort = function()
        start_aborted = true
      end,
    }
  end,
  register_on_key = function()
    start_registered = true
    return function() end
  end,
})
assert(pcall(function()
  failing_start:start()
end) == false)
assert(failing_start:status().state == "stopped", "failed start must roll back state")
assert(failing_start:status().pending_events == 0, "failed start must discard its incomplete boundary")
assert(start_aborted == true, "failed start must quarantine or remove its partial file")
assert(start_registered == false, "failed session_start must not attach input collection")

local registration_aborted = false
local registration_failure = collector.new({
  new_session_id = function()
    return "session-registration-failure"
  end,
  open_session = function()
    return {
      write = function() end,
      abort = function()
        registration_aborted = true
      end,
    }
  end,
  register_on_key = function()
    error("registration failed")
  end,
})
assert(pcall(function()
  registration_failure:start()
end) == false)
assert(registration_failure:status().state == "stopped")
assert(registration_aborted == true)

local stop_attempt = 0
local stopped_events = {}
local finish_count = 0
local failing_stop = collector.new({
  new_session_id = function()
    return "session-stop-failure"
  end,
  open_session = function()
    return {
      write = function(_, lines)
        stop_attempt = stop_attempt + 1
        if stop_attempt == 2 then
          error("temporary write failure")
        end
        for _, line in ipairs(lines) do
          table.insert(stopped_events, vim.json.decode(line))
        end
      end,
      flush = function() end,
      finish = function()
        finish_count = finish_count + 1
      end,
      abort = function() end,
    }
  end,
  register_on_key = function()
    return function() end
  end,
})
failing_stop:start()
assert(pcall(function()
  failing_stop:stop()
end) == false)
assert(failing_stop:status().state == "stopping")
assert(failing_stop:stop() == true, "a failed stop must be retryable")
assert(#stopped_events == 2)
assert(stopped_events[1].event_type == "session_start")
assert(stopped_events[2].event_type == "session_end")
assert(finish_count == 1)

local storage = require("key-insights.storage")
local temporary_directory = vim.fn.tempname()
local store = storage.new({ directory = temporary_directory })
local first_session = store:open_session("storage-one")
first_session:write({ schema.encode(schema.session_start("storage-one")) })
local second_session = store:open_session("storage-two")
second_session:write({ schema.encode(schema.session_start("storage-two")) })
second_session:write({ schema.encode(schema.session_end("storage-two", 1)) })
second_session:finish()

local complete_logs = vim.fn.glob(temporary_directory .. "/*.jsonl", false, true)
local incomplete_logs = vim.fn.glob(temporary_directory .. "/*.jsonl.part", false, true)
assert(#complete_logs == 1, "only finalized sessions must be analyzer inputs")
assert(#incomplete_logs == 1, "crashed sessions must remain quarantined")
assert(vim.deep_equal(vim.fn.readfile(complete_logs[1]), {
  vim.trim(schema.encode(schema.session_start("storage-two"))),
  vim.trim(schema.encode(schema.session_end("storage-two", 1))),
}))
local permissions = vim.uv.fs_stat(complete_logs[1]).mode % 512
assert(permissions == 384, "collector logs must be readable only by their owner")
assert(pcall(function()
  store:open_session("storage-two")
end) == false, "a finalized session ID must never be reused or overwritten")
first_session:abort()
vim.fn.delete(temporary_directory, "rf")

local short_write_chunks = {}
local fake_fs = {
  fs_stat = function()
    return nil, "ENOENT"
  end,
  fs_open = function()
    return 7
  end,
  fs_scandir = function()
    return {}
  end,
  fs_scandir_next = function()
    return nil
  end,
  fs_fchmod = function()
    return true
  end,
  fs_write = function(_, payload)
    local length = math.min(2, #payload)
    table.insert(short_write_chunks, string.sub(payload, 1, length))
    return length
  end,
  fs_fsync = function()
    return true
  end,
  fs_close = function()
    return true
  end,
  fs_rename = function()
    return true
  end,
  fs_unlink = function()
    return true
  end,
}
local short_store = storage.new({
  directory = "/virtual/key-insights",
  fs = fake_fs,
  mkdir = function()
    return 1
  end,
})
local short_session = short_store:open_session("short-write")
short_session:write({ "abcdef\n" })
short_session:finish()
assert(table.concat(short_write_chunks) == "abcdef\n", "short writes must be retried to completion")

local interrupted_chunks = {}
local interrupted_calls = 0
local interrupted_fs = vim.tbl_extend("force", fake_fs, {
  fs_write = function(_, payload)
    interrupted_calls = interrupted_calls + 1
    if interrupted_calls == 2 then
      return nil, "temporary write failure"
    end
    local length = math.min(2, #payload)
    table.insert(interrupted_chunks, string.sub(payload, 1, length))
    return length
  end,
})
local interrupted_store = storage.new({
  directory = "/virtual/key-insights",
  fs = interrupted_fs,
  mkdir = function()
    return 1
  end,
})
local interrupted_session = interrupted_store:open_session("interrupted-write")
assert(pcall(function()
  interrupted_session:write({ "abcdef\n" })
end) == false)
interrupted_session:write({ "abcdef\n" })
interrupted_session:finish()
assert(table.concat(interrupted_chunks) == "abcdef\n", "write retries must resume after partial progress")

local close_next_descriptor = 0
local close_session_descriptor = nil
local closed_descriptors = {}
local close_rename_calls = 0
local close_error_fs = vim.tbl_extend("force", fake_fs, {
  fs_open = function(path)
    close_next_descriptor = close_next_descriptor + 1
    if string.match(path, "%.jsonl%.part$") ~= nil then
      close_session_descriptor = close_next_descriptor
    end
    return close_next_descriptor
  end,
  fs_close = function(descriptor)
    closed_descriptors[descriptor] = (closed_descriptors[descriptor] or 0) + 1
    if descriptor == close_session_descriptor then
      return nil, "EIO after descriptor release"
    end
    return true
  end,
  fs_rename = function()
    close_rename_calls = close_rename_calls + 1
    return true
  end,
})
local close_error_store = storage.new({
  directory = "/virtual/key-insights",
  fs = close_error_fs,
  mkdir = function()
    return 1
  end,
})
local close_error_session = close_error_store:open_session("close-error")
close_error_session:write({ "complete\n" })
assert(pcall(function()
  close_error_session:finish()
end) == false)
close_error_session:finish()
assert(closed_descriptors[close_session_descriptor] == 1, "a possibly stale session descriptor must not be closed twice")
assert(close_rename_calls == 1, "retry must proceed directly to publication")

local hard_link_calls = 0
local no_hard_link_fs = vim.tbl_extend("force", fake_fs, {
  fs_link = function()
    hard_link_calls = hard_link_calls + 1
    return nil, "ENOTSUP"
  end,
})
local portable_store = storage.new({
  directory = "/virtual/key-insights",
  fs = no_hard_link_fs,
  mkdir = function()
    return 1
  end,
})
local portable_session = portable_store:open_session("portable-finalize")
portable_session:write({ "complete\n" })
portable_session:finish()
assert(hard_link_calls == 0, "session finalization must not depend on hard-link support")

local stat_error_unlinks = {}
local stat_error_fs = vim.tbl_extend("force", fake_fs, {
  fs_stat = function()
    return nil, "EIO"
  end,
  fs_unlink = function(path)
    table.insert(stat_error_unlinks, path)
    return true
  end,
})
local stat_error_store = storage.new({
  directory = "/virtual/key-insights",
  fs = stat_error_fs,
  mkdir = function()
    return 1
  end,
})
assert(pcall(function()
  stat_error_store:open_session("stat-error")
end) == false)
assert(#stat_error_unlinks == 1, "failed final-path lookup must release its session reservation")
assert(string.match(stat_error_unlinks[1], "%.lock$") ~= nil)

local next_descriptor = 0
local directory_descriptor = nil
local synced_descriptors = {}
local durable_fs = vim.tbl_extend("force", fake_fs, {
  fs_open = function(path)
    next_descriptor = next_descriptor + 1
    if path == "/virtual/key-insights" then
      directory_descriptor = next_descriptor
    end
    return next_descriptor
  end,
  fs_fsync = function(descriptor)
    synced_descriptors[descriptor] = true
    return true
  end,
})
local durable_store = storage.new({
  directory = "/virtual/key-insights",
  fs = durable_fs,
  mkdir = function()
    return 1
  end,
})
local durable_session = durable_store:open_session("durable-publish")
durable_session:write({ "complete\n" })
durable_session:finish()
assert(directory_descriptor ~= nil, "finalization must open the parent directory")
assert(synced_descriptors[directory_descriptor] == true, "finalization must fsync the parent directory")

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
local api = require("key-insights")
api.setup({ storage = { directory = command_log_directory } })
vim.cmd.KeyInsightsStart()
vim.cmd.KeyInsightsStop()
local command_logs = vim.fn.glob(command_log_directory .. "/*.jsonl", false, true)
assert(#command_logs == 1)
local command_events = vim.fn.readfile(command_logs[1])
assert(#command_events == 2)
local command_start = vim.json.decode(command_events[1])
local command_end = vim.json.decode(command_events[2])
assert(command_start.event_type == "session_start")
assert(command_end.event_type == "session_end")
assert(command_start.session_id == command_end.session_id)
vim.fn.delete(command_log_directory, "rf")

print("Lua collector contract: ok")
