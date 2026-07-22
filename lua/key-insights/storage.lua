local M = {}
local Storage = {}
Storage.__index = Storage
local SessionStorage = {}
SessionStorage.__index = SessionStorage

local OWNER_READ_WRITE = 384 -- 0600
local OWNER_DIRECTORY = 448 -- 0700
local MAX_SESSION_ID_BYTES = 128

local function default_directory()
  return vim.fs.joinpath(vim.fn.stdpath("state"), "key-insights", "sessions")
end

local function validate_session_id(session_id)
  assert(type(session_id) == "string" and session_id ~= "", "session ID must be a non-empty string")
  assert(#session_id <= MAX_SESSION_ID_BYTES, "session ID exceeds the storage limit")
  assert(string.match(session_id, "^[%w_-]+$") ~= nil, "session ID contains unsafe path characters")
end

local function path_exists(fs, path)
  local stat, stat_error = fs.fs_stat(path)
  if stat ~= nil then
    return true
  end
  if stat_error ~= nil and not string.find(tostring(stat_error), "ENOENT", 1, true) then
    error(stat_error)
  end
  return false
end

function M.new(options)
  local config = options or {}
  local directory = config.directory or default_directory()
  assert(type(directory) == "string" and directory ~= "", "storage directory must be a non-empty string")
  return setmetatable({
    directory = directory,
    _fs = config.fs or vim.uv,
    _mkdir = config.mkdir or vim.fn.mkdir,
  }, Storage)
end

function Storage:open_session(session_id)
  validate_session_id(session_id)
  local directory_created = self._mkdir(self.directory, "p", OWNER_DIRECTORY)
  assert(directory_created >= 0, "failed to create collector log directory")

  local partial_path = vim.fs.joinpath(self.directory, session_id .. ".jsonl.part")
  local final_path = vim.fs.joinpath(self.directory, session_id .. ".jsonl")
  local lock_path = vim.fs.joinpath(self.directory, session_id .. ".lock")
  local lock_descriptor, lock_error = self._fs.fs_open(lock_path, "wx", OWNER_READ_WRITE)
  assert(lock_descriptor ~= nil, lock_error or "collector session ID is already reserved")
  local lock_close_ok, lock_close_error = self._fs.fs_close(lock_descriptor)
  if not lock_close_ok then
    self._fs.fs_unlink(lock_path)
    error(lock_close_error or "failed to reserve collector session ID")
  end

  local lookup_ok, final_exists = pcall(path_exists, self._fs, final_path)
  if not lookup_ok then
    self._fs.fs_unlink(lock_path)
    error(final_exists, 0)
  end
  if final_exists then
    self._fs.fs_unlink(lock_path)
    error("collector session log already exists")
  end

  local descriptor, open_error = self._fs.fs_open(partial_path, "wx", OWNER_READ_WRITE)
  if descriptor == nil then
    self._fs.fs_unlink(lock_path)
    error(open_error or "failed to create collector session log")
  end

  local chmod_ok, chmod_error = self._fs.fs_fchmod(descriptor, OWNER_READ_WRITE)
  if not chmod_ok then
    self._fs.fs_close(descriptor)
    self._fs.fs_unlink(partial_path)
    self._fs.fs_unlink(lock_path)
    error(chmod_error or "failed to protect collector log permissions")
  end

  return setmetatable({
    _descriptor = descriptor,
    _directory = self.directory,
    _directory_synced = false,
    _final_path = final_path,
    _finished = false,
    _fs = self._fs,
    _lock_path = lock_path,
    _offset = 0,
    _partial_path = partial_path,
    _published = false,
    _unlocked = false,
    _write_consumed = 0,
    _write_payload = nil,
  }, SessionStorage)
end

function SessionStorage:_sync_directory()
  local descriptor, open_error = self._fs.fs_open(self._directory, "r", 0)
  assert(descriptor ~= nil, open_error or "failed to open collector session directory")
  local sync_ok, sync_error = self._fs.fs_fsync(descriptor)
  local close_ok, close_error = self._fs.fs_close(descriptor)
  assert(sync_ok, sync_error or "failed to make collector session publication durable")
  assert(close_ok, close_error or "failed to close collector session directory")
end

function SessionStorage:write(lines)
  assert(self._descriptor ~= nil, "collector session log is not open")
  assert(type(lines) == "table", "lines must be a list")
  local payload = table.concat(lines)
  if self._write_payload == nil then
    self._write_payload = payload
    self._write_consumed = 0
  else
    assert(payload == self._write_payload, "collector write retry must use the same batch")
  end

  while self._write_consumed < #self._write_payload do
    local remaining = string.sub(self._write_payload, self._write_consumed + 1)
    local bytes_written, write_error = self._fs.fs_write(self._descriptor, remaining, self._offset)
    assert(
      bytes_written ~= nil and bytes_written > 0 and bytes_written <= #remaining,
      write_error or "failed to make progress writing collector log"
    )
    self._write_consumed = self._write_consumed + bytes_written
    self._offset = self._offset + bytes_written
  end

  self._write_payload = nil
  self._write_consumed = 0
end

function SessionStorage:flush()
  if self._descriptor == nil then
    return
  end
  local ok, flush_error = self._fs.fs_fsync(self._descriptor)
  assert(ok, flush_error or "failed to flush collector session log")
end

function SessionStorage:finish()
  if self._finished then
    return
  end

  if self._descriptor ~= nil then
    self:flush()
    local descriptor = self._descriptor
    self._descriptor = nil
    local close_ok, close_error = self._fs.fs_close(descriptor)
    assert(close_ok, close_error or "failed to close collector session log")
  end

  if not self._published then
    local rename_ok, rename_error = self._fs.fs_rename(self._partial_path, self._final_path)
    assert(rename_ok, rename_error or "failed to publish collector session log")
    self._published = true
  end

  if not self._unlocked then
    local unlock_ok, unlock_error = self._fs.fs_unlink(self._lock_path)
    assert(unlock_ok, unlock_error or "failed to release collector session reservation")
    self._unlocked = true
  end

  if not self._directory_synced then
    self:_sync_directory()
    self._directory_synced = true
  end
  self._finished = true
end

function SessionStorage:abort()
  if self._descriptor ~= nil then
    self._fs.fs_close(self._descriptor)
    self._descriptor = nil
  end
  self._fs.fs_unlink(self._partial_path)
  self._fs.fs_unlink(self._lock_path)
end

return M
