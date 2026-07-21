local M = {}
local Storage = {}
Storage.__index = Storage

local OWNER_READ_WRITE = 384 -- 0600
local OWNER_DIRECTORY = 448 -- 0700

local function default_path()
  return vim.fs.joinpath(vim.fn.stdpath("state"), "key-insights", "events.jsonl")
end

function M.new(options)
  local config = options or {}
  local path = config.path or default_path()
  assert(type(path) == "string" and path ~= "", "storage path must be a non-empty string")
  return setmetatable({ path = path }, Storage)
end

function Storage:write(lines)
  assert(type(lines) == "table", "lines must be a list")
  if #lines == 0 then
    return
  end

  local directory = vim.fs.dirname(self.path)
  local directory_created = vim.fn.mkdir(directory, "p", OWNER_DIRECTORY)
  assert(directory_created >= 0, "failed to create collector log directory")

  local descriptor, open_error = vim.uv.fs_open(self.path, "a", OWNER_READ_WRITE)
  assert(descriptor ~= nil, open_error or "failed to open collector log")

  local chmod_ok, chmod_error = vim.uv.fs_fchmod(descriptor, OWNER_READ_WRITE)
  if not chmod_ok then
    vim.uv.fs_close(descriptor)
    error(chmod_error or "failed to protect collector log permissions")
  end

  local payload = table.concat(lines)
  local bytes_written, write_error = vim.uv.fs_write(descriptor, payload, -1)
  local close_ok, close_error = vim.uv.fs_close(descriptor)
  assert(bytes_written == #payload, write_error or "failed to write complete collector log batch")
  assert(close_ok, close_error or "failed to close collector log")
end

return M
