local keymap_snapshot = require("key-insights.keymap_snapshot")

local M = {}
local Payload = {}
Payload.__index = Payload

local MAX_ENCODED_BYTES = 1024 * 1024

function M.new(options, dependencies)
  local settings = options or {}
  local deps = dependencies or {}
  return setmetatable({
    _collect_snapshot = deps.collect_snapshot or function()
      return keymap_snapshot.collect({ options = rawget(settings, "collector_options") })
    end,
    _encode_snapshot = deps.encode_snapshot or keymap_snapshot.encode,
  }, Payload)
end

function Payload:collect()
  local collect_ok, model = pcall(self._collect_snapshot)
  if not collect_ok or model == nil then
    return nil, "snapshot_payload:collection_failed"
  end
  local encode_ok, encoded = pcall(self._encode_snapshot, model)
  if not encode_ok or type(encoded) ~= "string" or #encoded > MAX_ENCODED_BYTES then
    return nil, "snapshot_payload:encoding_failed"
  end
  return encoded
end

return M
