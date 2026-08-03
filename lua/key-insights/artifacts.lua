local M = {}

local FILE_PREFIX = "nvim-key-insights-"
local FILE_PREFIX_PATTERN = "nvim%-key%-insights%-"
local MAX_SESSION_ID_BYTES = 128
local OWNER_READ_WRITE = 384 -- 0600
local OWNER_DIRECTORY = 448 -- 0700
local PERMISSION_AND_SPECIAL_BITS = 4096
local SUFFIXES = {
  { extension = ".jsonl.part", kind = "partial" },
  { extension = ".jsonl", kind = "finalized" },
  { extension = ".lock", kind = "lock" },
}

local function valid_session_id(session_id)
  return type(session_id) == "string"
    and session_id ~= ""
    and #session_id <= MAX_SESSION_ID_BYTES
    and string.match(session_id, "^[A-Za-z0-9_-]+$") ~= nil
end

function M.validate_session_id(session_id)
  assert(type(session_id) == "string" and session_id ~= "", "session ID must be a non-empty string")
  assert(#session_id <= MAX_SESSION_ID_BYTES, "session ID exceeds the storage limit")
  assert(valid_session_id(session_id), "session ID contains unsafe path characters")
end

function M.parse(name, include_legacy)
  if type(name) ~= "string" then
    return nil
  end
  for _, suffix in ipairs(SUFFIXES) do
    local extension_pattern = suffix.extension:gsub("%.", "%%.")
    local session_id = string.match(name, "^" .. FILE_PREFIX_PATTERN .. "(.+)" .. extension_pattern .. "$")
    if valid_session_id(session_id) then
      return { kind = suffix.kind, legacy = false, session_id = session_id }
    end
  end
  if include_legacy then
    local session_id = string.match(name, "^([0-9a-f]+)%.jsonl$")
    if session_id ~= nil and #session_id == 32 then
      return { kind = "finalized", legacy = true, session_id = session_id }
    end
  end
  return nil
end

function M.name(session_id, suffix)
  M.validate_session_id(session_id)
  return FILE_PREFIX .. session_id .. suffix
end

function M.identity(stat)
  if stat == nil then
    return nil
  end
  local modified = stat.mtime or {}
  return table.concat({
    tostring(stat.type),
    tostring(stat.dev),
    tostring(stat.ino),
    tostring(stat.mode),
    tostring(stat.nlink),
    tostring(stat.uid),
    tostring(stat.size),
    tostring(modified.sec),
    tostring(modified.nsec),
  }, ":")
end

function M.directory_identity(stat)
  if stat == nil then
    return nil
  end
  return table.concat({
    tostring(stat.type),
    tostring(stat.dev),
    tostring(stat.ino),
    tostring(stat.mode),
    tostring(stat.uid),
  }, ":")
end

function M.current_user_id(fs)
  if type(fs) == "table" and type(fs.getuid) == "function" then
    return fs.getuid()
  end
  return nil
end

function M.is_private_file(stat, user_id)
  return stat ~= nil
    and type(user_id) == "number"
    and stat.type == "file"
    and stat.nlink == 1
    and type(stat.mode) == "number"
    and stat.mode % PERMISSION_AND_SPECIAL_BITS == OWNER_READ_WRITE
    and stat.uid == user_id
end

function M.is_private_directory(stat, user_id)
  return stat ~= nil
    and type(user_id) == "number"
    and stat.type == "directory"
    and type(stat.mode) == "number"
    and stat.mode % PERMISSION_AND_SPECIAL_BITS == OWNER_DIRECTORY
    and stat.uid == user_id
end

return M
