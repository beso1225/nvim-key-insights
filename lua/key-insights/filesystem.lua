local M = {}

local native_ffi = nil
local native_linkat = nil
local native_openat = nil
local native_unlinkat = nil
do
  local loaded, ffi = pcall(require, "ffi")
  if loaded then
    pcall(ffi.cdef, [[
      int linkat(int olddirfd, const char *oldpath, int newdirfd, const char *newpath, int flags);
      int openat(int dirfd, const char *pathname, int flags, ...);
      int unlinkat(int dirfd, const char *pathname, int flags);
    ]])
    local link_found, linkat = pcall(function() return ffi.C.linkat end)
    local open_found, openat = pcall(function() return ffi.C.openat end)
    local unlink_found, unlinkat = pcall(function() return ffi.C.unlinkat end)
    if link_found or open_found or unlink_found then
      native_ffi = ffi
    end
    if link_found then
      native_linkat = linkat
    end
    if open_found then
      native_openat = openat
    end
    if unlink_found then
      native_unlinkat = unlinkat
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

function M.open_child_exclusive(directory_descriptor, name, mode)
  if type(directory_descriptor) ~= "number" or not valid_child(name) or type(mode) ~= "number" then
    return nil, "invalid descriptor-relative open target"
  end
  if native_openat == nil then
    return nil, "descriptor-relative open is unavailable on this platform"
  end
  local constants = vim.uv.constants
  local flags = constants.O_WRONLY + constants.O_CREAT + constants.O_EXCL
  if constants.O_CLOEXEC ~= nil then
    flags = flags + constants.O_CLOEXEC
  end
  local descriptor = native_openat(directory_descriptor, name, flags, mode)
  if descriptor >= 0 then
    return tonumber(descriptor)
  end
  return nil, "descriptor-relative open failed with errno " .. tostring(native_ffi.errno())
end

function M.publish_child_exclusive(directory_descriptor, staging_name, final_name)
  if type(directory_descriptor) ~= "number" or not valid_child(staging_name) or not valid_child(final_name) then
    return nil, "invalid descriptor-relative publication target"
  end
  if native_linkat == nil then
    return nil, "descriptor-relative publication is unavailable on this platform"
  end
  if native_linkat(directory_descriptor, staging_name, directory_descriptor, final_name, 0) ~= 0 then
    return nil, "descriptor-relative publication failed with errno " .. tostring(native_ffi.errno())
  end
  if native_unlinkat(directory_descriptor, staging_name, 0) == 0 then
    return true
  end
  native_unlinkat(directory_descriptor, final_name, 0)
  return nil, "descriptor-relative staging cleanup failed with errno " .. tostring(native_ffi.errno())
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
