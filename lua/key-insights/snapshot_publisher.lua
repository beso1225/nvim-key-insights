local filesystem = require("key-insights.filesystem")
local keymap_snapshot = require("key-insights.keymap_snapshot")

local M = {}
local Publisher = {}
Publisher.__index = Publisher

local OWNER_READ_WRITE = 384 -- 0600
local MAX_ENCODED_BYTES = 1024 * 1024

local function is_enoent(error_message)
  return error_message ~= nil and string.find(tostring(error_message), "ENOENT", 1, true) ~= nil
end

local function same_directory(left, right)
  return left ~= nil
    and right ~= nil
    and left.type == "directory"
    and right.type == "directory"
    and left.dev ~= nil
    and left.ino ~= nil
    and left.dev == right.dev
    and left.ino == right.ino
end

local function write_all(fs, descriptor, payload)
  local offset = 0
  while offset < #payload do
    local remaining = string.sub(payload, offset + 1)
    local written = fs.fs_write(descriptor, remaining, offset)
    if type(written) ~= "number" or written <= 0 or written > #remaining then
      return false
    end
    offset = offset + written
  end
  return true
end

local function default_suffix()
  local entropy = table.concat({
    tostring(vim.uv.hrtime()),
    tostring(vim.fn.getpid()),
    tostring(math.random()),
  }, ":")
  return string.sub(vim.fn.sha256(entropy), 1, 32)
end

function M.new(options, dependencies)
  local settings = options or {}
  local output_directory = type(settings) == "table" and rawget(settings, "output_directory") or nil
  assert(type(output_directory) == "string" and output_directory ~= "", "snapshot output directory must be non-empty")
  local deps = dependencies or {}
  local fs = deps.fs or vim.uv
  local collector_options = rawget(settings, "collector_options")
  return setmetatable({
    _anchored = deps.fs == nil or fs == vim.uv,
    _collect_snapshot = deps.collect_snapshot or function()
      return keymap_snapshot.collect({ options = collector_options })
    end,
    _encode_snapshot = deps.encode_snapshot or keymap_snapshot.encode,
    _fs = fs,
    _name_suffix = deps.name_suffix or default_suffix,
    _output_directory = output_directory,
    _published = {},
  }, Publisher)
end

function Publisher:publish()
  local collect_ok, model = pcall(self._collect_snapshot)
  if not collect_ok or model == nil then
    return nil, "snapshot_publisher:collection_failed"
  end
  local encode_ok, encoded = pcall(self._encode_snapshot, model)
  if not encode_ok or type(encoded) ~= "string" or #encoded > MAX_ENCODED_BYTES then
    return nil, "snapshot_publisher:encoding_failed"
  end
  local suffix_ok, suffix = pcall(self._name_suffix)
  if not suffix_ok
    or type(suffix) ~= "string"
    or #suffix < 1
    or #suffix > 64
    or string.match(suffix, "^[a-z0-9][a-z0-9-]*$") == nil
  then
    return nil, "snapshot_publisher:invalid_name"
  end

  local before = self._fs.fs_lstat(self._output_directory)
  if before == nil or before.type ~= "directory" then
    return nil, "snapshot_publisher:directory_changed"
  end
  local directory_descriptor = filesystem.open_read(self._fs, self._output_directory)
  if directory_descriptor == nil then
    return nil, "snapshot_publisher:directory_changed"
  end
  local opened = self._fs.fs_fstat(directory_descriptor)
  if not same_directory(before, opened) then
    self._fs.fs_close(directory_descriptor)
    return nil, "snapshot_publisher:directory_changed"
  end

  local name = "keymap-snapshot-" .. suffix .. ".json"
  local final_path = vim.fs.joinpath(self._output_directory, name)
  local staging_name = name .. ".tmp"
  local staging_path = vim.fs.joinpath(self._output_directory, staging_name)
  local function close_directory()
    self._fs.fs_close(directory_descriptor)
  end
  local existing, lookup_error = self._fs.fs_lstat(final_path)
  if existing ~= nil or not is_enoent(lookup_error) then
    close_directory()
    return nil, "snapshot_publisher:target_exists"
  end

  local descriptor = nil
  if self._anchored then
    descriptor = filesystem.open_child_exclusive(directory_descriptor, staging_name, OWNER_READ_WRITE)
  else
    descriptor = self._fs.fs_open(staging_path, "wx", OWNER_READ_WRITE)
  end
  if descriptor == nil then
    close_directory()
    return nil, "snapshot_publisher:write_failed"
  end
  local protected = self._fs.fs_fchmod(descriptor, OWNER_READ_WRITE)
  local written = protected and write_all(self._fs, descriptor, encoded)
  local synced = written and self._fs.fs_fsync(descriptor)
  local closed = self._fs.fs_close(descriptor)
  if not protected or not written or not synced or not closed then
    if self._anchored then
      filesystem.unlink_child(directory_descriptor, staging_name)
    else
      self._fs.fs_unlink(staging_path)
    end
    close_directory()
    return nil, "snapshot_publisher:write_failed"
  end

  local current = self._fs.fs_lstat(self._output_directory)
  local target = self._fs.fs_lstat(final_path)
  if not same_directory(opened, current) then
    if self._anchored then
      filesystem.unlink_child(directory_descriptor, staging_name)
    else
      self._fs.fs_unlink(staging_path)
    end
    close_directory()
    return nil, "snapshot_publisher:directory_changed"
  end
  if target ~= nil then
    if self._anchored then
      filesystem.unlink_child(directory_descriptor, staging_name)
    else
      self._fs.fs_unlink(staging_path)
    end
    close_directory()
    return nil, "snapshot_publisher:target_exists"
  end
  local renamed = nil
  if self._anchored then
    renamed = filesystem.publish_child_exclusive(directory_descriptor, staging_name, name)
  else
    renamed = self._fs.fs_rename(staging_path, final_path)
  end
  if not renamed then
    if self._anchored then
      filesystem.unlink_child(directory_descriptor, staging_name)
    else
      self._fs.fs_unlink(staging_path)
    end
    close_directory()
    return nil, "snapshot_publisher:publish_failed"
  end
  local after = self._fs.fs_lstat(self._output_directory)
  local published = same_directory(opened, after) and self._fs.fs_lstat(final_path) or nil
  local directory_synced = self._fs.fs_fsync(directory_descriptor)
  if published == nil
    or published.type ~= "file"
    or published.nlink ~= 1
    or published.mode % 512 ~= OWNER_READ_WRITE
    or not directory_synced
  then
    if self._anchored then
      filesystem.unlink_child(directory_descriptor, name)
    else
      self._fs.fs_unlink(final_path)
    end
    self._fs.fs_close(directory_descriptor)
    return nil, "snapshot_publisher:publish_failed"
  end
  self._fs.fs_close(directory_descriptor)
  local identity = filesystem.stat_identity(published)
  self._published[final_path] = identity
  -- A close error after the file and directory have both been synchronized does
  -- not make the already-published immutable snapshot unusable.
  return final_path, identity
end

function Publisher:remove(path, expected_identity)
  if type(path) ~= "string"
    or vim.fs.dirname(path) ~= self._output_directory
    or string.match(vim.fs.basename(path), "^keymap%-snapshot%-%l[%l%d%-]*%.json$") == nil
  then
    return false
  end
  expected_identity = expected_identity or self._published[path]
  if type(expected_identity) ~= "string" then
    return false
  end
  local before = self._fs.fs_lstat(self._output_directory)
  local target = self._fs.fs_lstat(path)
  if before == nil
    or before.type ~= "directory"
    or target == nil
    or target.type ~= "file"
    or target.nlink ~= 1
    or target.mode % 512 ~= OWNER_READ_WRITE
    or filesystem.stat_identity(target) ~= expected_identity
  then
    return false
  end
  local directory_descriptor = filesystem.open_read(self._fs, self._output_directory)
  if directory_descriptor == nil then
    return false
  end
  local opened = self._fs.fs_fstat(directory_descriptor)
  local removed = false
  if same_directory(before, opened) then
    local current = self._fs.fs_lstat(path)
    if filesystem.stat_identity(current) == expected_identity then
      if self._anchored then
        removed = filesystem.unlink_child(directory_descriptor, vim.fs.basename(path)) == true
      else
        removed = self._fs.fs_unlink(path) == true
      end
    end
  end
  if removed then
    self._fs.fs_fsync(directory_descriptor)
  end
  self._fs.fs_close(directory_descriptor)
  if removed then
    self._published[path] = nil
  end
  return removed
end

return M
