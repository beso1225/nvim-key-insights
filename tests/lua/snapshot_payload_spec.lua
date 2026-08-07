local snapshot_payload = require("key-insights.snapshot_payload")

local encoded = '{"snapshot_version":1,"mappings":[]}\n'
local model = { snapshot_version = 1, mappings = {} }

local instance = snapshot_payload.new({}, {
  collect_snapshot = function() return model end,
  encode_snapshot = function(value)
    assert(value == model)
    return encoded
  end,
})
assert(instance:collect() == encoded)

local collection_failure = snapshot_payload.new({}, {
  collect_snapshot = function()
    return nil, "secret-buffer-name"
  end,
})
local collection_result, collection_error = collection_failure:collect()
assert(collection_result == nil and collection_error == "snapshot_payload:collection_failed")
assert(string.find(collection_error, "secret", 1, true) == nil)

local encoding_failure = snapshot_payload.new({}, {
  collect_snapshot = function() return model end,
  encode_snapshot = function()
    return nil, "secret-mapping"
  end,
})
local encoding_result, encoding_error = encoding_failure:collect()
assert(encoding_result == nil and encoding_error == "snapshot_payload:encoding_failed")
assert(string.find(encoding_error, "secret", 1, true) == nil)

local oversized = snapshot_payload.new({}, {
  collect_snapshot = function() return model end,
  encode_snapshot = function() return string.rep("x", 1024 * 1024 + 1) end,
})
local oversized_result, oversized_error = oversized:collect()
assert(oversized_result == nil and oversized_error == "snapshot_payload:encoding_failed")

print("Lua sanitized snapshot payload contract: ok")
