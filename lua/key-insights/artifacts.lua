local M = {}

local FILE_PREFIX = "nvim-key-insights-"
local FILE_PREFIX_PATTERN = "nvim%-key%-insights%-"
local MAX_SESSION_ID_BYTES = 128
local OWNER_READ_WRITE = 384 -- 0600
local OWNER_DIRECTORY = 448 -- 0700
local QUARANTINE_PREFIX = ".nvim-key-insights-quarantine-"
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

function M.identity_digest(identity)
  assert(type(identity) == "string" and identity ~= "", "artifact identity must be non-empty")
  return string.sub(vim.fn.sha256(identity), 1, 32)
end

function M.quarantine_name(original_name, expected_identity, nonce)
  local nested = M.parse_quarantine(original_name)
  if nested ~= nil then
    original_name = nested.original_name
  end
  assert(M.parse(original_name, true) ~= nil, "quarantine source must be a collector artifact")
  assert(type(nonce) == "string" and string.match(nonce, "^[0-9a-f]+$") ~= nil and #nonce == 16)
  return QUARANTINE_PREFIX .. M.identity_digest(expected_identity) .. "-" .. nonce .. "-" .. original_name
end

function M.parse_quarantine(name)
  if type(name) ~= "string" then
    return nil
  end
  local digest, nonce, original_name = string.match(
    name,
    "^%.nvim%-key%-insights%-quarantine%-([0-9a-f]+)%-([0-9a-f]+)%-(.+)$"
  )
  if digest == nil or #digest ~= 32 or nonce == nil or #nonce ~= 16 then
    return nil
  end
  local original = M.parse(original_name, true)
  if original == nil then
    return nil
  end
  return {
    identity_digest = digest,
    legacy = original.legacy,
    original_kind = original.kind,
    original_name = original_name,
    session_id = original.session_id,
  }
end

function M.is_recoverable_quarantine(name, stat)
  local parsed = M.parse_quarantine(name)
  return parsed ~= nil and M.identity_digest(M.identity(stat)) == parsed.identity_digest
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
