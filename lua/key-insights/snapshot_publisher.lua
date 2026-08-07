local filesystem = require("key-insights.filesystem")
local keymap_snapshot = require("key-insights.keymap_snapshot")

local M = {}
local Publisher = {}
Publisher.__index = Publisher

local OWNER_READ_WRITE = 384 -- 0600
local MAX_ENCODED_BYTES = 1024 * 1024
local MAX_RETAINED_SNAPSHOTS = 16

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

local function default_slot_seed(digest)
  return tonumber(string.sub(digest, 1, 8), 16)
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
    _slot_seed = deps.slot_seed or default_slot_seed,
    _output_directory = output_directory,
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
  local digest_ok, digest = pcall(vim.fn.sha256, encoded)
  if not digest_ok or type(digest) ~= "string" or #digest ~= 64 or string.match(digest, "^[0-9a-f]+$") == nil then
    return nil, "snapshot_publisher:encoding_failed"
  end
  local seed_ok, seed = pcall(self._slot_seed, digest, encoded)
  if not seed_ok or type(seed) ~= "number" or seed < 0 or seed >= math.huge or seed ~= math.floor(seed) then
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

  local function close_directory()
    self._fs.fs_close(directory_descriptor)
  end
  for offset = 0, MAX_RETAINED_SNAPSHOTS - 1 do
    local slot = (seed + offset) % MAX_RETAINED_SNAPSHOTS
    local name = string.format("keymap-snapshot-%02x.json", slot)
    local final_path = vim.fs.joinpath(self._output_directory, name)
    local existing = self._fs.fs_lstat(final_path)
    if existing ~= nil then
      local existing_contents = nil
      if existing.type == "file" and existing.nlink == 1 and existing.mode % 512 == OWNER_READ_WRITE then
        existing_contents = filesystem.read_bounded(self._fs, final_path, MAX_ENCODED_BYTES)
      end
      if existing_contents == encoded then
        close_directory()
        return final_path, filesystem.stat_identity(existing), "sha256:" .. digest
      end
    else
      local descriptor = nil
      if self._anchored then
        descriptor = filesystem.open_child_exclusive(directory_descriptor, name, OWNER_READ_WRITE)
      else
        descriptor = self._fs.fs_open(final_path, "wx", OWNER_READ_WRITE)
      end
      if descriptor ~= nil then
        local protected = self._fs.fs_fchmod(descriptor, OWNER_READ_WRITE)
        local written = protected and write_all(self._fs, descriptor, encoded)
        local synced = written and self._fs.fs_fsync(descriptor)
        local published = synced and self._fs.fs_fstat(descriptor) or nil
        local current = self._fs.fs_lstat(self._output_directory)
        local directory_unchanged = same_directory(opened, current)
        local directory_synced = published ~= nil and directory_unchanged and self._fs.fs_fsync(directory_descriptor)
        local closed = self._fs.fs_close(descriptor)
        if not directory_unchanged then
          close_directory()
          return nil, "snapshot_publisher:directory_changed"
        end
        if not protected or not written or not synced or published == nil or not directory_synced or not closed then
          close_directory()
          return nil, "snapshot_publisher:write_failed"
        end
        close_directory()
        return final_path, filesystem.stat_identity(published), "sha256:" .. digest
      end
    end
  end
  close_directory()
  return nil, "snapshot_publisher:retention_limit"
end

return M
