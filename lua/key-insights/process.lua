local M = {}
local MAX_CAPTURED_STDOUT = 256 * 1024 + 1
local MAX_CAPTURED_STDERR = 8 * 1024

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

function M.run(argv, callback, stdin)
  local capture_stdout, stdout = bounded_capture(MAX_CAPTURED_STDOUT)
  local capture_stderr, stderr = bounded_capture(MAX_CAPTURED_STDERR)
  return vim.system(
    argv,
    { text = true, stdin = stdin, stdout = capture_stdout, stderr = capture_stderr },
    vim.schedule_wrap(function(result)
      result.stdout = stdout()
      result.stderr = stderr()
      callback(result)
    end)
  )
end

return M
