local artifacts = require("key-insights.artifacts")

local M = {}
local Purge = {}
Purge.__index = Purge

local DEFAULT_MAX_ENTRIES = 4096
local DEFAULT_MAX_TARGETS = 512
local MAX_LOCK_METADATA_BYTES = 1024

local function is_enoent(error_message)
  return error_message ~= nil and string.find(tostring(error_message), "ENOENT", 1, true) ~= nil
end

local function default_is_process_alive(pid)
  local result, error_message = vim.uv.kill(pid, 0)
  if result == 0 then
    return true
  end
  return not is_enoent(error_message) and string.find(tostring(error_message), "ESRCH", 1, true) == nil
end

local function default_confirm(message)
  return vim.fn.confirm(message, "&Purge\n&Cancel", 2) == 1
end

local function default_notify(message, level)
  vim.notify("key-insights: " .. message, level)
end

local function positive_integer(value, name)
  assert(
    type(value) == "number" and value < math.huge and value == math.floor(value) and value > 0,
    name .. " must be a positive integer"
  )
end

local function same_identity(stat, expected)
  return artifacts.identity(stat) == expected
end

local function close_checked(fs, descriptor)
  local closed, close_error = fs.fs_close(descriptor)
  assert(closed, close_error or "failed to close collector artifact")
end

function M.new(options, dependencies)
  local config = options or {}
  local deps = dependencies or {}
  assert(
    type(config.directory) == "string" and config.directory ~= "",
    "purge directory must be a non-empty string"
  )
  local max_entries = deps.max_entries or DEFAULT_MAX_ENTRIES
  local max_targets = deps.max_targets or DEFAULT_MAX_TARGETS
  positive_integer(max_entries, "purge max_entries")
  positive_integer(max_targets, "purge max_targets")
  return setmetatable({
    _active_session_id = config.active_session_id or function()
      return nil
    end,
    _confirm = deps.confirm or default_confirm,
    _directory = config.directory,
    _fs = deps.fs or vim.uv,
    _include_legacy = config.include_legacy == true,
    _is_process_alive = deps.is_process_alive or default_is_process_alive,
    _max_entries = max_entries,
    _max_targets = max_targets,
    _notify_fn = deps.notify or default_notify,
    _user_id = deps.user_id == nil and artifacts.current_user_id(vim.uv) or deps.user_id,
  }, Purge)
end

function Purge:_inspect_directory()
  local before, stat_error = self._fs.fs_lstat(self._directory)
  if before == nil and is_enoent(stat_error) then
    return nil
  end
  assert(before ~= nil, "collector directory is unavailable")
  assert(artifacts.is_private_directory(before, self._user_id), "collector directory is not privately owned")
  local descriptor = self._fs.fs_open(self._directory, "r", 0)
  assert(descriptor ~= nil, "failed to open collector directory")
  local opened, inspect_error = self._fs.fs_fstat(descriptor)
  local valid = opened ~= nil
    and artifacts.is_private_directory(opened, self._user_id)
    and artifacts.directory_identity(opened) == artifacts.directory_identity(before)
  close_checked(self._fs, descriptor)
  assert(valid, inspect_error or "collector directory changed while opening")
  return artifacts.directory_identity(before)
end

function Purge:_read_lock(entry)
  local stat = entry.stat
  if stat.size <= 0 or stat.size > MAX_LOCK_METADATA_BYTES then
    return nil
  end
  local descriptor, open_error = self._fs.fs_open(entry.path, "r", 0)
  if descriptor == nil then
    return nil, open_error
  end
  local opened, inspect_error = self._fs.fs_fstat(descriptor)
  if not artifacts.is_private_file(opened, self._user_id) or not same_identity(opened, entry.identity) then
    close_checked(self._fs, descriptor)
    return nil, inspect_error or "collector lock changed while opening"
  end
  local payload, read_error = self._fs.fs_read(descriptor, stat.size, 0)
  local after_read = self._fs.fs_fstat(descriptor)
  close_checked(self._fs, descriptor)
  if payload == nil or not same_identity(after_read, entry.identity) then
    return nil, read_error
  end
  local decoded_ok, metadata = pcall(vim.json.decode, payload)
  if not decoded_ok
    or type(metadata) ~= "table"
    or metadata.version ~= 1
    or type(metadata.pid) ~= "number"
    or metadata.pid >= math.huge
    or metadata.pid ~= math.floor(metadata.pid)
    or metadata.pid <= 0
  then
    return nil
  end
  return metadata
end

function Purge:_lock_state(entry)
  local metadata = self:_read_lock(entry)
  if metadata == nil then
    return "unknown"
  end
  local ok, alive = pcall(self._is_process_alive, metadata.pid)
  if not ok or type(alive) ~= "boolean" then
    return "unknown"
  end
  return alive and "live" or "stale"
end

function Purge:_scan()
  local directory_identity = self:_inspect_directory()
  if directory_identity == nil then
    return nil, {}, 0
  end
  local request = self._fs.fs_scandir(self._directory)
  assert(request ~= nil, "failed to scan collector directory")
  local entries = {}
  local skipped = 0
  local scanned = 0
  while true do
    local name = self._fs.fs_scandir_next(request)
    if name == nil then
      break
    end
    scanned = scanned + 1
    assert(scanned <= self._max_entries, "collector directory exceeds the purge scan limit")
    local parsed = artifacts.parse(name, self._include_legacy)
    if parsed == nil then
      skipped = skipped + 1
    else
      local entry_path = vim.fs.joinpath(self._directory, name)
      local stat, stat_error = self._fs.fs_lstat(entry_path)
      if stat == nil then
        if not is_enoent(stat_error) then
          error("failed to inspect collector artifact")
        end
      elseif not artifacts.is_private_file(stat, self._user_id) then
        skipped = skipped + 1
      else
        local entry = {
          identity = artifacts.identity(stat),
          kind = parsed.kind,
          name = name,
          path = entry_path,
          session_id = parsed.session_id,
          stat = stat,
        }
        entries[parsed.session_id] = entries[parsed.session_id] or {}
        table.insert(entries[parsed.session_id], entry)
      end
    end
  end
  assert(self:_inspect_directory() == directory_identity, "collector directory changed while scanning")
  return directory_identity, entries, skipped
end

function Purge:preview()
  local directory_identity, sessions, skipped = self:_scan()
  local targets = {}
  local protected = {}
  local active_session_id = self._active_session_id()
  for session_id, entries in pairs(sessions) do
    local lock = nil
    for _, entry in ipairs(entries) do
      if entry.kind == "lock" then
        lock = entry
        break
      end
    end
    local is_protected = session_id == active_session_id
    if not is_protected and lock ~= nil then
      is_protected = self:_lock_state(lock) ~= "stale"
    end
    for _, entry in ipairs(entries) do
      entry.stat = nil
      if is_protected then
        table.insert(protected, entry)
      else
        table.insert(targets, entry)
        assert(#targets <= self._max_targets, "collector artifacts exceed the purge target limit")
      end
    end
  end
  local function by_name(left, right)
    return left.name < right.name
  end
  table.sort(targets, by_name)
  table.sort(protected, by_name)
  return {
    directory_identity = directory_identity,
    protected = protected,
    skipped = skipped,
    targets = targets,
  }
end

function Purge:_directory_unchanged(expected)
  if expected == nil then
    return self._fs.fs_lstat(self._directory) == nil
  end
  local stat = self._fs.fs_lstat(self._directory)
  return artifacts.is_private_directory(stat, self._user_id) and artifacts.directory_identity(stat) == expected
end

function Purge:_session_is_protected(session_id)
  if self._active_session_id() == session_id then
    return true
  end
  local lock_path = vim.fs.joinpath(self._directory, artifacts.name(session_id, ".lock"))
  local stat, stat_error = self._fs.fs_lstat(lock_path)
  if stat == nil then
    if is_enoent(stat_error) then
      return false
    end
    return true
  end
  if not artifacts.is_private_file(stat, self._user_id) then
    return true
  end
  return self:_lock_state({
    identity = artifacts.identity(stat),
    path = lock_path,
    stat = stat,
  }) ~= "stale"
end

function Purge:apply(preview)
  assert(type(preview) == "table" and type(preview.targets) == "table", "invalid purge preview")
  assert(self:_directory_unchanged(preview.directory_identity), "collector directory changed before purge")
  local current = self:preview()
  assert(current.directory_identity == preview.directory_identity, "collector directory changed before purge")
  local eligible = {}
  local currently_protected = {}
  for _, entry in ipairs(current.targets) do
    eligible[entry.name] = entry
  end
  for _, entry in ipairs(current.protected) do
    currently_protected[entry.name] = true
  end
  local result = {
    cancelled = false,
    failed = 0,
    protected = #current.protected,
    removed = 0,
    skipped = current.skipped,
  }
  local ordered = vim.deepcopy(preview.targets)
  table.sort(ordered, function(left, right)
    if (left.kind == "lock") ~= (right.kind == "lock") then
      return left.kind ~= "lock"
    end
    return left.name < right.name
  end)
  for _, entry in ipairs(ordered) do
    assert(self:_directory_unchanged(preview.directory_identity), "collector directory changed during purge")
    local current_entry = eligible[entry.name]
    local stat = self._fs.fs_lstat(entry.path)
    if currently_protected[entry.name] then
      -- The refreshed preview already counted this artifact as protected.
    elseif self:_session_is_protected(current_entry and current_entry.session_id or entry.session_id) then
      result.protected = result.protected + 1
    elseif current_entry == nil
      or not artifacts.is_private_file(stat, self._user_id)
      or not same_identity(stat, entry.identity)
      or current_entry.identity ~= entry.identity
    then
      result.failed = result.failed + 1
    else
      local unlinked, unlink_error = self._fs.fs_unlink(entry.path)
      if unlinked then
        result.removed = result.removed + 1
      else
        result.failed = result.failed + 1
        if not is_enoent(unlink_error) then
          self._notify_fn("failed to remove " .. entry.name, vim.log.levels.ERROR)
        end
      end
    end
  end
  assert(self:_directory_unchanged(preview.directory_identity), "collector directory changed during purge")
  return result
end

function Purge:run(force)
  local preview = self:preview()
  if #preview.targets == 0 then
    local result = {
      cancelled = false,
      failed = 0,
      protected = #preview.protected,
      removed = 0,
      skipped = preview.skipped,
    }
    local message = string.format(
      "purge removed 0; protected %d; skipped %d; failed 0",
      result.protected,
      result.skipped
    )
    self._notify_fn(message, vim.log.levels.INFO)
    return result
  end
  if not force then
    local target_names = vim.tbl_map(function(entry)
      return "  " .. entry.name
    end, preview.targets)
    local message = string.format("Purge %d collector artifact(s)?\n%s", #target_names, table.concat(target_names, "\n"))
    if not self._confirm(message) then
      return {
        cancelled = true,
        failed = 0,
        protected = #preview.protected,
        removed = 0,
        skipped = preview.skipped,
      }
    end
  end
  local result = self:apply(preview)
  self._notify_fn(string.format(
    "purge removed %d; protected %d; skipped %d; failed %d",
    result.removed,
    result.protected,
    result.skipped,
    result.failed
  ), result.failed == 0 and vim.log.levels.INFO or vim.log.levels.WARN)
  return result
end

return M
