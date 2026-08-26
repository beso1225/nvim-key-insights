local M = {}
local artifacts = require("key-insights.artifacts")

function M.is_absolute_path(path, separator)
  if type(path) ~= "string" or path == "" then
    return false
  end
  local platform_separator = separator or string.sub(package.config, 1, 1)
  if platform_separator == "\\" then
    return string.match(path, "^%a:[/\\]") ~= nil
      or string.sub(path, 1, 2) == "\\\\"
      or string.sub(path, 1, 2) == "//"
  end
  return string.sub(path, 1, 1) == "/"
end

local native_ffi = nil
local native_unlinkat = nil
local native_rename_child_noreplace = nil
do
  local loaded, ffi = pcall(require, "ffi")
  if loaded then
    pcall(ffi.cdef, [[
      int unlinkat(int dirfd, const char *pathname, int flags);
      int renameat2(int olddirfd, const char *oldpath, int newdirfd, const char *newpath, unsigned int flags);
      int renameatx_np(int fromfd, const char *from, int tofd, const char *to, unsigned int flags);
    ]])
    local unlink_found, unlinkat = pcall(function() return ffi.C.unlinkat end)
    if unlink_found then
      native_ffi = ffi
    end
    if unlink_found then
      native_unlinkat = unlinkat
    end
    local system = vim.uv.os_uname().sysname
    if system == "Linux" then
      local found, renameat2 = pcall(function() return ffi.C.renameat2 end)
      if found then
        native_rename_child_noreplace = function(descriptor, source, destination)
          return renameat2(descriptor, source, descriptor, destination, 1)
        end
      end
    elseif system == "Darwin" then
      local found, renameatx_np = pcall(function() return ffi.C.renameatx_np end)
      if found then
        native_rename_child_noreplace = function(descriptor, source, destination)
          return renameatx_np(descriptor, source, descriptor, destination, 4)
        end
      end
    end
  end
end

local function valid_child(name)
  return type(name) == "string"
    and name ~= ""
    and name ~= "."
    and name ~= ".."
    and string.find(name, "/", 1, true) == nil
    and string.find(name, "\0", 1, true) == nil
end

function M.open_read(fs, path)
  local flags = vim.uv.constants.O_RDONLY + vim.uv.constants.O_NONBLOCK
  return fs.fs_open(path, flags, 0)
end

function M.unlink_child(directory_descriptor, name)
  if type(directory_descriptor) ~= "number" or not valid_child(name) then
    return nil, "invalid descriptor-relative unlink target"
  end
  if native_unlinkat == nil then
    return nil, "descriptor-relative unlink is unavailable on this platform"
  end
  if native_unlinkat(directory_descriptor, name, 0) == 0 then
    return true
  end
  local error_number = native_ffi.errno()
  if error_number == 2 then
    return nil, "ENOENT: descriptor-relative unlink target is missing"
  end
  local description = type(vim.uv.strerror) == "function" and vim.uv.strerror(error_number) or nil
  description = description or ("errno " .. tostring(error_number))
  return nil, "descriptor-relative unlink failed: " .. description
end

local function rename_child_noreplace(directory_descriptor, source, destination)
  if type(directory_descriptor) ~= "number"
    or not valid_child(source)
    or not valid_child(destination)
    or native_rename_child_noreplace == nil
  then
    return nil, "atomic descriptor-relative rename is unavailable"
  end
  if native_rename_child_noreplace(directory_descriptor, source, destination) == 0 then
    return true
  end
  local error_number = native_ffi.errno()
  if error_number == 2 then
    return nil, "ENOENT: descriptor-relative rename source is missing"
  end
  if error_number == 17 then
    return nil, "EEXIST: descriptor-relative rename destination exists"
  end
  local description = type(vim.uv.strerror) == "function" and vim.uv.strerror(error_number) or nil
  return nil, "descriptor-relative rename failed: " .. (description or ("errno " .. tostring(error_number)))
end

local function quarantine_name(name, expected_identity, attempt)
  local entropy = table.concat({ name, tostring(vim.uv.hrtime()), tostring(math.random()), tostring(attempt) }, ":")
  return artifacts.quarantine_name(name, expected_identity, string.sub(vim.fn.sha256(entropy), 1, 16))
end

function M.unlink_child_if_identity(fs, directory_descriptor, path, name, expected_identity, identity, operations)
  if type(identity) ~= "function" then
    return nil, "identity-aware unlink requires an identity function"
  end
  local config = operations or {}
  local rename_child = config.rename_child or rename_child_noreplace
  local quarantine = config.quarantine_name
  local moved, move_error = nil, nil
  for attempt = 1, 8 do
    quarantine = quarantine or quarantine_name(name, expected_identity, attempt)
    moved, move_error = rename_child(directory_descriptor, name, quarantine)
    if moved or not tostring(move_error):find("EEXIST", 1, true) then
      break
    end
    quarantine = nil
  end
  if not moved then
    return nil, move_error or "failed to quarantine identity-aware unlink target"
  end
  local quarantine_path = vim.fs.joinpath(vim.fs.dirname(path), quarantine)
  local stat, stat_error = fs.fs_lstat(quarantine_path)
  if stat == nil or identity(stat) ~= expected_identity then
    local restored, restore_error = rename_child(directory_descriptor, quarantine, name)
    if not restored then
      return nil, (stat_error or "identity-aware unlink target changed")
        .. "; quarantined entry was preserved: "
        .. tostring(restore_error)
    end
    return nil, stat_error or "identity-aware unlink target changed"
  end
  local unlink_child_operation = config.unlink_child or M.unlink_child
  local unlinked, unlink_error = unlink_child_operation(directory_descriptor, quarantine)
  if unlinked then
    return true
  end
  local restored, restore_error = rename_child(directory_descriptor, quarantine, name)
  if not restored then
    return nil, tostring(unlink_error) .. "; quarantined entry was preserved: " .. tostring(restore_error)
  end
  return nil, unlink_error
end

function M.stat_identity(stat)
  if stat == nil then
    return nil
  end
  local modified = stat.mtime or {}
  return table.concat({
    tostring(stat.type),
    tostring(stat.dev),
    tostring(stat.ino),
    tostring(stat.size),
    tostring(modified.sec),
    tostring(modified.nsec),
  }, ":")
end

function M.read_bounded(fs, path, maximum_bytes)
  local before, stat_error = fs.fs_lstat(path)
  if before == nil then
    return nil, stat_error or "file is missing"
  end
  if before.type ~= "file" then
    return nil, "path is not a regular file"
  end
  if before.size > maximum_bytes then
    return nil, "file exceeds its size limit"
  end
  local descriptor, open_error = M.open_read(fs, path)
  if descriptor == nil then
    return nil, open_error or "failed to open file"
  end
  local opened, inspect_error = fs.fs_fstat(descriptor)
  if opened == nil or opened.type ~= "file" or opened.dev ~= before.dev or opened.ino ~= before.ino then
    fs.fs_close(descriptor)
    return nil, inspect_error or "file changed while opening"
  end
  if opened.size > maximum_bytes then
    fs.fs_close(descriptor)
    return nil, "file exceeds its size limit"
  end

  local chunks = {}
  local offset = 0
  local read_failed = false
  local read_error = nil
  while offset <= maximum_bytes do
    local requested = math.min(64 * 1024, maximum_bytes + 1 - offset)
    local chunk
    chunk, read_error = fs.fs_read(descriptor, requested, offset)
    if chunk == nil then
      read_failed = true
      break
    end
    if #chunk == 0 then
      break
    end
    table.insert(chunks, chunk)
    offset = offset + #chunk
  end
  local after_read, after_error = fs.fs_fstat(descriptor)
  local closed, close_error = fs.fs_close(descriptor)
  if read_failed or not closed then
    return nil, read_error or close_error or "failed to read file"
  end
  if M.stat_identity(after_read) ~= M.stat_identity(opened) then
    return nil, after_error or "file changed while reading"
  end
  if offset > maximum_bytes then
    return nil, "file exceeds its size limit"
  end
  return table.concat(chunks)
end

return M
