local M = {}
local MAX_CAPTURED_STDOUT = 256 * 1024 + 1
local MAX_CONFIGURED_STDOUT = 1024 * 1024 + 1
local MAX_CAPTURED_STDERR = 8 * 1024
local DEFAULT_TIMEOUT_MS = 120 * 1000
local IS_WINDOWS = package.config:sub(1, 1) == "\\"

local function compatible_environment(environment, clear_environment)
  local version = vim.version()
  if not clear_environment or version.major ~= 0 or version.minor ~= 10 then
    return environment
  end
  local entries = {}
  for key, value in pairs(environment or {}) do
    assert(type(key) == "string" and type(value) == "string", "process environment must contain strings")
    table.insert(entries, key .. "=" .. value)
  end
  table.sort(entries)
  return entries
end

function M.supports_process_groups()
  return not IS_WINDOWS
end

local function terminate_process_group(handle, signal, fallback_to_direct)
  if not IS_WINDOWS and type(handle.pid) == "number" then
    local called, result = pcall(vim.uv.kill, -handle.pid, signal)
    if called and result == 0 then
      return true
    end
  end
  if fallback_to_direct ~= false and type(handle.kill) == "function" then
    return pcall(handle.kill, handle, signal)
  end
  return false
end

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
  local requested_stdout = run_options and run_options.max_stdout_bytes or MAX_CAPTURED_STDOUT
  local stdout_limit = math.min(math.max(1, requested_stdout), MAX_CONFIGURED_STDOUT)
  local capture_stdout, stdout = bounded_capture(stdout_limit)
  local capture_stderr, stderr = bounded_capture(MAX_CAPTURED_STDERR)
  local timeout_ms = run_options and run_options.timeout_ms or DEFAULT_TIMEOUT_MS
  local finished = false
  local timer = nil
  local monitor = nil
  local handle = nil
  local function stop_timers()
    if timer ~= nil then
      pcall(timer.stop, timer)
      pcall(timer.close, timer)
      timer = nil
    end
    if monitor ~= nil then
      pcall(monitor.stop, monitor)
      pcall(monitor.close, monitor)
      monitor = nil
    end
  end
  local function complete(result)
    if finished then
      return
    end
    if handle ~= nil then
      terminate_process_group(handle, 9, false)
    end
    finished = true
    stop_timers()
    result.stdout = stdout()
    result.stderr = stderr()
    callback(result)
  end
  local system_options = {
    text = true,
    stdin = stdin,
    stdout = capture_stdout,
    stderr = capture_stderr,
    detach = true,
  }
  if run_options ~= nil then
    system_options.clear_env = run_options.clear_env == true
    if run_options.env ~= nil or system_options.clear_env then
      system_options.env = compatible_environment(run_options.env, system_options.clear_env)
    end
  end
  handle = vim.system(argv, system_options, vim.schedule_wrap(complete))
  if not finished and type(timeout_ms) == "number" and timeout_ms > 0 and timeout_ms < math.huge then
    timer = vim.uv.new_timer()
    timer:start(math.floor(timeout_ms), 0, function()
      if not finished and handle ~= nil then
        terminate_process_group(handle, 9)
      end
    end)
  end
  if not finished and not IS_WINDOWS and type(handle.pid) == "number" then
    monitor = vim.uv.new_timer()
    monitor:start(10, 10, function()
      local called, result = pcall(vim.uv.kill, handle.pid, 0)
      if not finished and (not called or result ~= 0) then
        terminate_process_group(handle, 9, false)
      end
    end)
  end
  return {
    pid = handle.pid,
    kill = function(_, signal)
      return terminate_process_group(handle, signal)
    end,
  }
end

return M
