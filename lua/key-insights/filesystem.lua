local M = {}

function M.open_read(fs, path)
  local flags = vim.uv.constants.O_RDONLY + vim.uv.constants.O_NONBLOCK
  return fs.fs_open(path, flags, 0)
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
