local M = {}
local MAX_CAPTURED_STDOUT = 256 * 1024 + 1
local MAX_CAPTURED_STDERR = 8 * 1024
local DEFAULT_TIMEOUT_MS = 120 * 1000

local function bounded_capture(limit)
  local chunks = {}
  local bytes = 0
  local overflowed = false
  return function(_, data)
    if data == nil or overflowed then
      return
    end
    local remaining = limit - bytes
    if #data > remaining then
      if remaining > 0 then
        table.insert(chunks, string.sub(data, 1, remaining))
        bytes = limit
      end
      overflowed = true
      return
    end
    table.insert(chunks, data)
    bytes = bytes + #data
  end, function()
    return table.concat(chunks)
  end
end

function M.run(argv, callback, stdin, run_options)
  local capture_stdout, stdout = bounded_capture(MAX_CAPTURED_STDOUT)
  local capture_stderr, stderr = bounded_capture(MAX_CAPTURED_STDERR)
  local timeout_ms = run_options and run_options.timeout_ms or DEFAULT_TIMEOUT_MS
  local finished = false
  local timer = nil
  local handle = nil
  local function stop_timer()
    if timer ~= nil then
      pcall(timer.stop, timer)
      pcall(timer.close, timer)
      timer = nil
    end
  end
  local function complete(result)
    if finished then
      return
    end
    finished = true
    stop_timer()
    result.stdout = stdout()
    result.stderr = stderr()
    callback(result)
  end
  handle = vim.system(
    argv,
    { text = true, stdin = stdin, stdout = capture_stdout, stderr = capture_stderr },
    vim.schedule_wrap(complete)
  )
  if not finished and type(timeout_ms) == "number" and timeout_ms > 0 and timeout_ms < math.huge then
    timer = vim.uv.new_timer()
    timer:start(math.floor(timeout_ms), 0, function()
      if not finished and handle ~= nil and type(handle.kill) == "function" then
        pcall(handle.kill, handle, 15)
        timer:start(1000, 0, function()
          if not finished and handle ~= nil and type(handle.kill) == "function" then
            pcall(handle.kill, handle, 9)
          end
        end)
      end
    end)
  end
  return handle
end

return M
