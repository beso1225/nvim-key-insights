local artifacts = require("key-insights.artifacts")
local filesystem = require("key-insights.filesystem")

local M = {}
local Storage = {}
Storage.__index = Storage
local SessionStorage = {}
SessionStorage.__index = SessionStorage

local OWNER_READ_WRITE = 384 -- 0600
local OWNER_DIRECTORY = 448 -- 0700
local DAY_SECONDS = 24 * 60 * 60
local LOCK_METADATA_VERSION = 1
local MAX_LOCK_METADATA_BYTES = 1024
local MAX_RETENTION_SCAN_ENTRIES = 8192
local MAX_RETENTION_DELETIONS_PER_PASS = 512
local DEFAULT_RETENTION = {
  max_age_days = 30,
  max_sessions = 100,
}

local function default_directory()
  return vim.fs.joinpath(vim.fn.stdpath("state"), "key-insights", "sessions")
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
    type(value) == "number" and value < math.huge and value == math.floor(value) and value > 0,
    name .. " must be a positive integer"
  )
end

local function default_is_process_alive(pid)
  local result, error_message = vim.uv.kill(pid, 0)
  if result == 0 then
    return true
  end
  return not is_enoent(error_message) and string.find(tostring(error_message), "ESRCH", 1, true) == nil
end

local function write_all(fs, descriptor, payload)
  local offset = 0
  while offset < #payload do
    local remaining = string.sub(payload, offset + 1)
    local bytes_written, write_error = fs.fs_write(descriptor, remaining, offset)
    assert(
      bytes_written ~= nil and bytes_written > 0 and bytes_written <= #remaining,
      write_error or "failed to write collector lock metadata"
    )
    offset = offset + bytes_written
  end
end

function M.new(options)
  local config = options or {}
  local directory_provider = config.default_directory or default_directory
  local collector_directory = directory_provider()
  local directory = config.directory or collector_directory
  local uses_default_directory = directory == collector_directory
  local retention = vim.tbl_extend("force", DEFAULT_RETENTION, config.retention or {})
  assert(type(directory) == "string" and directory ~= "", "storage directory must be a non-empty string")
  validate_positive_integer(retention.max_sessions, "storage.retention.max_sessions")
  validate_positive_integer(retention.max_age_days, "storage.retention.max_age_days")
  local max_retention_scan_entries = config.max_retention_scan_entries or MAX_RETENTION_SCAN_ENTRIES
  local max_retention_deletions_per_pass =
    config.max_retention_deletions_per_pass or MAX_RETENTION_DELETIONS_PER_PASS
  validate_positive_integer(max_retention_scan_entries, "retention scan entry limit")
  validate_positive_integer(max_retention_deletions_per_pass, "retention deletion limit")
  local process_id = config.process_id or vim.fn.getpid()
  validate_positive_integer(process_id, "collector process ID")
  local on_retention_error = config.on_retention_error or function()
    vim.notify("key-insights: retention cleanup was deferred", vim.log.levels.WARN)
  end
  assert(type(on_retention_error) == "function", "storage retention error handler must be a function")
  return setmetatable({
    directory = directory,
    _fs = config.fs or vim.uv,
    _include_legacy_logs = uses_default_directory,
    _is_process_alive = config.is_process_alive or default_is_process_alive,
    _max_retention_deletions_per_pass = max_retention_deletions_per_pass,
    _max_retention_scan_entries = max_retention_scan_entries,
    _mkdir = config.mkdir or vim.fn.mkdir,
    _now_seconds = config.now_seconds or os.time,
    _on_retention_error = on_retention_error,
    _process_id = process_id,
    _retention = retention,
    _unlink_child = config.unlink_child or function(descriptor, name, expected_identity, path)
      return filesystem.unlink_child_if_identity(
        config.fs or vim.uv,
        descriptor,
        path,
        name,
        expected_identity,
        artifacts.identity
      )
    end,
    _user_id = config.user_id == nil and artifacts.current_user_id(vim.uv) or config.user_id,
  }, Storage)
end

function Storage:includes_legacy_logs()
  return self._include_legacy_logs
end

function Storage:_entry_type(name, entry_type)
  if entry_type ~= nil then
    return entry_type
  end
  local path = vim.fs.joinpath(self.directory, name)
  local stat, stat_error = self._fs.fs_lstat(path)
  if stat ~= nil then
    return stat.type
  end
  if is_enoent(stat_error) then
    return nil
  end
  error(stat_error or "failed to inspect collector directory entry")
end

function Storage:_lock_owner_alive(path)
  local stat, stat_error = self._fs.fs_lstat(path)
  if stat == nil then
    if is_enoent(stat_error) then
      return false
    end
    error(stat_error or "failed to inspect collector lock")
  end
  if not artifacts.is_private_file(stat, self._user_id)
    or stat.size <= 0
    or stat.size > MAX_LOCK_METADATA_BYTES
  then
    return false
  end

  local descriptor, open_error = filesystem.open_read(self._fs, path)
  if descriptor == nil then
    if is_enoent(open_error) then
      return false
    end
    error(open_error or "failed to open collector lock")
  end
  local payload, read_error = self._fs.fs_read(descriptor, stat.size, 0)
  local opened, inspect_error = self._fs.fs_fstat(descriptor)
  local close_ok, close_error = self._fs.fs_close(descriptor)
  assert(close_ok, close_error or "failed to close collector lock")
  assert(payload ~= nil, read_error or "failed to read collector lock")
  if not artifacts.is_private_file(opened, self._user_id)
    or artifacts.identity(opened) ~= artifacts.identity(stat)
  then
    error(inspect_error or "collector lock changed while opening")
  end

  local decoded_ok, metadata = pcall(vim.json.decode, payload)
  if not decoded_ok
    or type(metadata) ~= "table"
    or metadata.version ~= LOCK_METADATA_VERSION
    or type(metadata.pid) ~= "number"
    or metadata.pid >= math.huge
    or metadata.pid ~= math.floor(metadata.pid)
    or metadata.pid <= 0
  then
    return false
  end
  return self._is_process_alive(metadata.pid) == true
end

function Storage:_retention_inventory()
  local request, scan_error = self._fs.fs_scandir(self.directory)
  assert(request ~= nil, scan_error or "failed to scan collector log directory")

  local logs = {}
  local lock_paths = {}
  local quarantines = {}
  local scanned_entries = 0
  while true do
    local name, entry_type = self._fs.fs_scandir_next(request)
    if name == nil then
      break
    end
    scanned_entries = scanned_entries + 1
    if scanned_entries > self._max_retention_scan_entries then
      error("collector retention scan entry limit exceeded")
    end
    entry_type = self:_entry_type(name, entry_type)
    local quarantine = entry_type == "file" and artifacts.parse_quarantine(name) or nil
    local parsed = entry_type == "file" and artifacts.parse(name, self._include_legacy_logs) or nil
    if quarantine ~= nil and (not quarantine.legacy or self._include_legacy_logs) then
      local path = vim.fs.joinpath(self.directory, name)
      local stat = self._fs.fs_lstat(path)
      if artifacts.is_private_file(stat, self._user_id) and artifacts.is_recoverable_quarantine(name, stat) then
        table.insert(quarantines, { identity = artifacts.identity(stat), name = name, path = path })
      end
    elseif parsed ~= nil and parsed.kind == "lock" then
      lock_paths[parsed.session_id] = vim.fs.joinpath(self.directory, name)
    elseif parsed ~= nil and parsed.kind == "finalized" then
      local path = vim.fs.joinpath(self.directory, name)
      local stat, stat_error = self._fs.fs_lstat(path)
      if artifacts.is_private_file(stat, self._user_id) then
        assert(stat.mtime ~= nil and type(stat.mtime.sec) == "number", "collector log mtime is unavailable")
        table.insert(logs, {
          identity = artifacts.identity(stat),
          modified_at = stat.mtime.sec,
          name = name,
          path = path,
          session_id = parsed.session_id,
        })
      elseif stat == nil and not is_enoent(stat_error) then
        error(stat_error or "failed to inspect collector log")
      end
    end
  end
  for _, log in ipairs(logs) do
    local lock_path = lock_paths[log.session_id]
    log.locked = lock_path ~= nil and self:_lock_owner_alive(lock_path)
  end
  table.sort(quarantines, function(left, right) return left.name < right.name end)
  return { logs = logs, quarantines = quarantines }
end

function Storage:_finalized_logs()
  return self:_retention_inventory().logs
end

function Storage:_unlink_log(log)
  local directory_stat, directory_error = self._fs.fs_lstat(self.directory)
  assert(
    artifacts.is_private_directory(directory_stat, self._user_id),
    directory_error or "collector directory changed since retention scan"
  )
  local descriptor, open_error = filesystem.open_read(self._fs, self.directory)
  assert(descriptor ~= nil, open_error or "failed to open collector directory for retention")
  local opened, inspect_error = self._fs.fs_fstat(descriptor)
  if not artifacts.is_private_directory(opened, self._user_id)
    or artifacts.directory_identity(opened) ~= artifacts.directory_identity(directory_stat)
  then
    local closed, close_error = self._fs.fs_close(descriptor)
    assert(closed, close_error or "failed to close changed collector directory")
    error(inspect_error or "collector directory changed while opening for retention")
  end
  local operation_ok, unlinked, unlink_error = pcall(
    self._unlink_child,
    descriptor,
    log.name,
    log.identity,
    log.path
  )
  local sync_ok, synced, sync_error = pcall(self._fs.fs_fsync, descriptor)
  local close_ok, closed, close_error = pcall(self._fs.fs_close, descriptor)
  assert(close_ok and closed, close_error or "failed to close collector directory after retention")
  assert(operation_ok, "collector retention deletion failed")
  assert(sync_ok, "failed to synchronize retention deletion")
  assert(synced == true or synced == 0, sync_error or "failed to synchronize retention deletion")
  if not unlinked and not is_enoent(unlink_error) then
    error("collector log changed since retention scan")
  end
end

function Storage:_recover_quarantines(deletions_remaining, candidates)
  local deferred = false
  for _, candidate in ipairs(candidates) do
    if deletions_remaining == 0 then
      deferred = true
      break
    end
    self:_unlink_log(candidate)
    deletions_remaining = deletions_remaining - 1
  end
  return deletions_remaining, deferred
end

function Storage:_prune(protected_path)
  local inventory = self:_retention_inventory()
  local deletions_remaining, deferred = self:_recover_quarantines(
    self._max_retention_deletions_per_pass,
    inventory.quarantines
  )
  if deferred then
    error("collector retention deletion limit exceeded")
  end
  local cutoff = self._now_seconds() - self._retention.max_age_days * DAY_SECONDS
  local retained = {}
  local logs = inventory.logs
  table.sort(logs, function(left, right)
    if left.modified_at == right.modified_at then
      return left.name < right.name
    end
    return left.modified_at < right.modified_at
  end)
  for _, log in ipairs(logs) do
    if log.path ~= protected_path and not log.locked and log.modified_at < cutoff then
      if deletions_remaining > 0 then
        self:_unlink_log(log)
        deletions_remaining = deletions_remaining - 1
      else
        deferred = true
        table.insert(retained, log)
      end
    else
      table.insert(retained, log)
    end
  end

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
    if deletions_remaining == 0 then
      deferred = true
      break
    end
    self:_unlink_log(retained[delete_index])
    deletions_remaining = deletions_remaining - 1
    table.remove(retained, delete_index)
  end
  if deferred then
    error("collector retention deletion limit exceeded")
  end
end

function Storage:open_session(session_id)
  artifacts.validate_session_id(session_id)
  local directory_created = self._mkdir(self.directory, "p", OWNER_DIRECTORY)
  assert(directory_created >= 0, "failed to create collector log directory")

  local partial_path = vim.fs.joinpath(self.directory, artifacts.name(session_id, ".jsonl.part"))
  local final_path = vim.fs.joinpath(self.directory, artifacts.name(session_id, ".jsonl"))
  local lock_path = vim.fs.joinpath(self.directory, artifacts.name(session_id, ".lock"))
  local lock_descriptor, lock_error = self._fs.fs_open(lock_path, "wx", OWNER_READ_WRITE)
  assert(lock_descriptor ~= nil, lock_error or "collector session ID is already reserved")
  local lock_payload = vim.json.encode({
    pid = self._process_id,
    version = LOCK_METADATA_VERSION,
  }) .. "\n"
  local lock_write_ok, lock_write_error = pcall(write_all, self._fs, lock_descriptor, lock_payload)
  if lock_write_ok then
    local sync_ok, sync_error = self._fs.fs_fsync(lock_descriptor)
    lock_write_ok = sync_ok == true or sync_ok == 0
    lock_write_error = sync_error or "failed to flush collector lock metadata"
  end
  local lock_close_ok, lock_close_error = self._fs.fs_close(lock_descriptor)
  if not lock_write_ok or not lock_close_ok then
    self._fs.fs_unlink(lock_path)
    error(lock_write_error or lock_close_error or "failed to reserve collector session ID")
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
    _on_retention_error = self._on_retention_error,
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
  local descriptor, open_error = filesystem.open_read(self._fs, self._directory)
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
    return true
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

  local retention_error = nil
  if not self._retention_done then
    local retention_ok, prune_error = pcall(self._prune)
    if retention_ok then
      self._retention_done = true
    else
      retention_error = tostring(prune_error)
    end
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
  if retention_error ~= nil then
    pcall(self._on_retention_error, retention_error)
  end
  return true
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
