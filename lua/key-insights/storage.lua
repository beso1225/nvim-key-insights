local M = {}
local Storage = {}
Storage.__index = Storage
local SessionStorage = {}
SessionStorage.__index = SessionStorage

local OWNER_READ_WRITE = 384 -- 0600
local OWNER_DIRECTORY = 448 -- 0700
local MAX_SESSION_ID_BYTES = 128
local DAY_SECONDS = 24 * 60 * 60
local DEFAULT_RETENTION = {
  max_age_days = 30,
  max_sessions = 100,
}

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

local function is_enoent(error_message)
  return error_message ~= nil and string.find(tostring(error_message), "ENOENT", 1, true) ~= nil
end

local function validate_positive_integer(value, name)
  assert(
    type(value) == "number" and value == math.floor(value) and value > 0,
    name .. " must be a positive integer"
  )
end

function M.new(options)
  local config = options or {}
  local directory = config.directory or default_directory()
  local retention = vim.tbl_extend("force", DEFAULT_RETENTION, config.retention or {})
  assert(type(directory) == "string" and directory ~= "", "storage directory must be a non-empty string")
  validate_positive_integer(retention.max_sessions, "storage.retention.max_sessions")
  validate_positive_integer(retention.max_age_days, "storage.retention.max_age_days")
  return setmetatable({
    directory = directory,
    _fs = config.fs or vim.uv,
    _mkdir = config.mkdir or vim.fn.mkdir,
    _now_seconds = config.now_seconds or os.time,
    _retention = retention,
  }, Storage)
end

function Storage:_finalized_logs()
  local request, scan_error = self._fs.fs_scandir(self.directory)
  assert(request ~= nil, scan_error or "failed to scan collector log directory")

  local logs = {}
  local locked_sessions = {}
  while true do
    local name, entry_type = self._fs.fs_scandir_next(request)
    if name == nil then
      break
    end
    if entry_type == "file" and string.match(name, "^[%w_-]+%.lock$") ~= nil then
      locked_sessions[string.sub(name, 1, -6)] = true
    elseif entry_type == "file" and string.match(name, "^[%w_-]+%.jsonl$") ~= nil then
      local path = vim.fs.joinpath(self.directory, name)
      local stat, stat_error = self._fs.fs_stat(path)
      if stat ~= nil then
        assert(stat.mtime ~= nil and type(stat.mtime.sec) == "number", "collector log mtime is unavailable")
        table.insert(logs, {
          modified_at = stat.mtime.sec,
          name = name,
          path = path,
        })
      elseif not is_enoent(stat_error) then
        error(stat_error or "failed to inspect collector log")
      end
    end
  end
  for _, log in ipairs(logs) do
    local session_id = string.sub(log.name, 1, -7)
    log.locked = locked_sessions[session_id] == true
  end
  return logs
end

function Storage:_unlink_log(path)
  local unlinked, unlink_error = self._fs.fs_unlink(path)
  if not unlinked and not is_enoent(unlink_error) then
    error(unlink_error or "failed to prune collector log")
  end
end

function Storage:_prune(protected_path)
  local cutoff = self._now_seconds() - self._retention.max_age_days * DAY_SECONDS
  local retained = {}
  for _, log in ipairs(self:_finalized_logs()) do
    if log.path ~= protected_path and not log.locked and log.modified_at < cutoff then
      self:_unlink_log(log.path)
    else
      table.insert(retained, log)
    end
  end

  table.sort(retained, function(left, right)
    if left.modified_at == right.modified_at then
      return left.name < right.name
    end
    return left.modified_at < right.modified_at
  end)

  while #retained > self._retention.max_sessions do
    local delete_index = nil
    for index, log in ipairs(retained) do
      if log.path ~= protected_path and not log.locked then
        delete_index = index
        break
      end
    end
    if delete_index == nil then
      break
    end
    self:_unlink_log(retained[delete_index].path)
    table.remove(retained, delete_index)
  end
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
    _retention_done = false,
    _unlocked = false,
    _write_consumed = 0,
    _write_payload = nil,
    _prune = function()
      self:_prune(final_path)
    end,
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

  if not self._retention_done then
    self._prune()
    self._retention_done = true
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
