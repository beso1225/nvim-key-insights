local snapshot = require("key-insights.keymap_snapshot")

local MODES = {
  n = "normal",
  x = "visual",
  o = "operator_pending",
}

local DEFAULT_LIMITS = {
  max_api_entries = 32,
  max_buffers = 8,
  max_encoded_bytes = 16 * 1024,
  max_lhs_tokens = 16,
  max_token_bytes = 256,
}

local function copy(value)
  return vim.deepcopy(value)
end

local function fake_sha256(value)
  return vim.fn.sha256(value)
end

local function entry(lhsraw, fields)
  return vim.tbl_extend("force", {
    lhs = "display-value-must-not-win",
    lhsraw = lhsraw,
    mode = "raw-mode-must-not-win",
  }, fields or {})
end

local function fake_dependencies(overrides)
  local dependencies = {
    get_buffer_keymaps = function()
      return {}
    end,
    get_global_keymaps = function()
      return {}
    end,
    is_buffer_loaded = function()
      return true
    end,
    is_buffer_excluded = function()
      return false
    end,
    is_buffer_sensitive = function()
      return false
    end,
    is_buffer_valid = function()
      return true
    end,
    keytrans = function(value)
      return value
    end,
    list_buffers = function()
      return {}
    end,
    sha256 = fake_sha256,
  }
  return vim.tbl_extend("force", dependencies, overrides or {})
end

local function collect(dependencies, limits)
  return snapshot.collect({ limits = limits or DEFAULT_LIMITS }, fake_dependencies(dependencies))
end

local function assert_failure(dependencies, limits, expected_error, secret)
  local result, error_code = collect(dependencies, limits)
  assert(result == nil, "a failed collection must not expose a partial snapshot")
  assert(error_code == expected_error, vim.inspect(error_code))
  if secret ~= nil then
    assert(string.find(vim.inspect(error_code), secret, 1, true) == nil)
  end
end

-- Canonicalization consumes lhsraw, never the display-oriented lhs field. The
-- injected keytrans results model aliases that have the same raw representation.
local raw_tab = "\t"
local raw_return = "\r"
local alias_keytrans = function(value)
  return ({
    [raw_tab] = "<Tab>",
    [raw_return] = "<CR>",
    [" "] = "<Space>",
  })[value]
end

local tab_from_ctrl_i = assert(snapshot.canonicalize_lhs(entry(raw_tab, { lhs = "<C-I>" }), {
  keytrans = alias_keytrans,
}, DEFAULT_LIMITS))
local tab_from_tab = assert(snapshot.canonicalize_lhs(entry(raw_tab, { lhs = "<Tab>" }), {
  keytrans = alias_keytrans,
}, DEFAULT_LIMITS))
assert(vim.deep_equal(tab_from_ctrl_i, { "<Tab>" }))
assert(vim.deep_equal(tab_from_ctrl_i, tab_from_tab))

local return_from_ctrl_m = assert(snapshot.canonicalize_lhs(entry(raw_return, { lhs = "<C-M>" }), {
  keytrans = alias_keytrans,
}, DEFAULT_LIMITS))
local return_from_cr = assert(snapshot.canonicalize_lhs(entry(raw_return, { lhs = "<CR>" }), {
  keytrans = alias_keytrans,
}, DEFAULT_LIMITS))
assert(vim.deep_equal(return_from_ctrl_m, { "<CR>" }))
assert(vim.deep_equal(return_from_ctrl_m, return_from_cr))
assert(vim.deep_equal(assert(snapshot.canonicalize_lhs(entry(" "), {
  keytrans = alias_keytrans,
}, DEFAULT_LIMITS)), { "<Space>" }))

local preimages = {}
local identity_dependencies = {
  sha256 = function(preimage)
    table.insert(preimages, preimage)
    return string.rep("a", 64)
  end,
}
local first_id = assert(snapshot.mapping_id("normal", "global", { "<Tab>", "c" }, identity_dependencies))
local second_id = assert(snapshot.mapping_id("normal", "global", { "<Tab>", "c" }, identity_dependencies))
assert(first_id == "mapping-v1:" .. string.rep("a", 64))
assert(second_id == first_id)
assert(preimages[1] == "10:mapping-v16:normal6:global1:25:<Tab>1:c")
assert(preimages[2] == preimages[1])

local function real_id(mode, scope, tokens)
  return assert(snapshot.mapping_id(mode, scope, tokens, { sha256 = fake_sha256 }))
end

assert(real_id("normal", "global", { "a", "b" }) ~= real_id("normal", "global", { "a" }))
assert(real_id("normal", "global", { "g", "g" }) ~= real_id("visual", "global", { "g", "g" }))
assert(real_id("normal", "global", { "g", "g" }) ~= real_id("normal", "buffer", { "g", "g" }))

local mapping_secret = "rhs-/private/credentials-and-control-\0"
local description_secret = "description-secret"
local callback_secret = "callback-secret"
local path_secret = "/private/project/init.lua"
local hostile = entry("gg", {
  buffer = 987,
  callback = function()
    return callback_secret
  end,
  desc = description_secret,
  lnum = 42,
  path = path_secret,
  rhs = mapping_secret,
  sid = 73,
})
local sanitized = assert(collect({
  get_global_keymaps = function(mode)
    return mode == "n" and { hostile } or {}
  end,
}))
assert(#sanitized.mappings == 1)
local sanitized_keys = vim.tbl_keys(sanitized.mappings[1])
table.sort(sanitized_keys)
assert(vim.deep_equal(sanitized_keys, { "lhs", "mapping_id", "mode", "scope" }))
assert(vim.deep_equal(sanitized.mappings[1].lhs, { "g", "g" }))
assert(sanitized.mappings[1].mapping_id == real_id("normal", "global", { "g", "g" }))
local sanitized_json = assert(snapshot.encode(sanitized, { max_encoded_bytes = DEFAULT_LIMITS.max_encoded_bytes }))
for _, secret in ipairs({ mapping_secret, description_secret, callback_secret, path_secret }) do
  assert(string.find(sanitized_json, secret, 1, true) == nil, secret)
end
for _, rejected_field in ipairs({ '"buffer":', '"callback":', '"desc":', '"lnum":', '"rhs":', '"sid":' }) do
  assert(string.find(sanitized_json, rejected_field, 1, true) == nil, rejected_field)
end

local queried_modes = {}
local mode_projection = assert(collect({
  get_global_keymaps = function(mode)
    table.insert(queried_modes, mode)
    return { entry(mode, { mode = "hostile-mode-" .. mode }) }
  end,
}))
assert(vim.deep_equal(queried_modes, { "n", "x", "o" }))
assert(#mode_projection.mappings == 3)
for index, mode in ipairs({ "normal", "operator_pending", "visual" }) do
  assert(mode_projection.mappings[index].mode == mode)
end

local duplicates = assert(collect({
  get_buffer_keymaps = function(buffer, mode)
    if mode == "n" and (buffer == 11 or buffer == 12) then
      return { entry("gg") }
    end
    return {}
  end,
  get_global_keymaps = function(mode)
    if mode ~= "n" then
      return {}
    end
    return {
      entry("z"),
      entry("aa"),
      entry("aa"),
      entry("gg"),
    }
  end,
  list_buffers = function()
    return { 12, 11 }
  end,
}))
assert(#duplicates.mappings == 4, "global and buffer scope must remain distinct")
assert(vim.deep_equal(vim.tbl_map(function(item)
  return table.concat({ item.mode, table.concat(item.lhs, ""), item.scope }, "/")
end, duplicates.mappings), {
  "normal/aa/global",
  "normal/gg/buffer",
  "normal/gg/global",
  "normal/z/global",
}))

local queried_buffers = {}
local eligible = assert(collect({
  get_buffer_keymaps = function(buffer)
    queried_buffers[buffer] = (queried_buffers[buffer] or 0) + 1
    return {}
  end,
  is_buffer_loaded = function(buffer)
    return buffer ~= 12
  end,
  is_buffer_excluded = function(buffer)
    return buffer == 15
  end,
  is_buffer_sensitive = function(buffer)
    return buffer == 13
  end,
  is_buffer_valid = function(buffer)
    return buffer ~= 11
  end,
  list_buffers = function()
    return { 11, 12, 13, 14, 15 }
  end,
}))
assert(eligible.snapshot_version == 1)
assert(queried_buffers[11] == nil)
assert(queried_buffers[12] == nil)
assert(queried_buffers[13] == nil)
assert(queried_buffers[14] == 3)
assert(queried_buffers[15] == nil)

assert_failure({
  list_buffers = function()
    return { 1, 2, 3 }
  end,
}, vim.tbl_extend("force", copy(DEFAULT_LIMITS), { max_buffers = 2 }), "keymap_snapshot:limit_exceeded")

assert_failure({
  get_global_keymaps = function(mode)
    return mode == "n" and { entry("a"), entry("b"), entry("c") } or {}
  end,
}, vim.tbl_extend("force", copy(DEFAULT_LIMITS), { max_api_entries = 2 }), "keymap_snapshot:limit_exceeded")

assert_failure({
  get_global_keymaps = function(mode)
    return mode == "n" and { entry("abc") } or {}
  end,
}, vim.tbl_extend("force", copy(DEFAULT_LIMITS), { max_lhs_tokens = 2 }), "keymap_snapshot:limit_exceeded")

assert_failure({
  get_global_keymaps = function(mode)
    return mode == "n" and { entry("<secret-too-long>") } or {}
  end,
}, vim.tbl_extend("force", copy(DEFAULT_LIMITS), { max_token_bytes = 4 }), "keymap_snapshot:limit_exceeded", "secret-too-long")

assert_failure({
  get_global_keymaps = function(mode)
    return mode == "n" and { entry("a") } or {}
  end,
}, vim.tbl_extend("force", copy(DEFAULT_LIMITS), { max_encoded_bytes = 1 }), "keymap_snapshot:limit_exceeded")

assert_failure({
  get_global_keymaps = function(mode)
    return mode == "n" and { entry("a"), entry("b") } or {}
  end,
  sha256 = function()
    return string.rep("f", 64)
  end,
}, DEFAULT_LIMITS, "keymap_snapshot:identity_conflict")

local invalid_id, invalid_id_error = snapshot.mapping_id("normal", "global", { "g" }, {
  sha256 = function()
    return "not-a-digest"
  end,
})
assert(invalid_id == nil and invalid_id_error == "keymap_snapshot:hash_failed")

local unsplit_id, unsplit_error = snapshot.mapping_id("normal", "global", { "/private/path" }, {
  sha256 = fake_sha256,
})
assert(unsplit_id == nil and unsplit_error == "keymap_snapshot:invalid_mapping")

local invalid_utf8, invalid_utf8_error = snapshot.canonicalize_lhs(entry("\255"), {
  keytrans = function(value)
    return value
  end,
}, DEFAULT_LIMITS)
assert(invalid_utf8 == nil and invalid_utf8_error == "keymap_snapshot:invalid_mapping")

local malformed_secret = "malformed-/private/secret"
assert_failure({
  get_global_keymaps = function(mode)
    return mode == "n" and { entry(nil, { lhs = malformed_secret, lhsraw = 42 }) } or {}
  end,
}, DEFAULT_LIMITS, "keymap_snapshot:invalid_mapping", malformed_secret)

local api_secret = "api-/private/secret"
assert_failure({
  get_global_keymaps = function(mode)
    if mode == "n" then
      return { entry("prior") }
    end
    error(api_secret)
  end,
}, DEFAULT_LIMITS, "keymap_snapshot:api_failed", api_secret)

local loaded_api_secret = "loaded-api-/private/secret"
assert_failure({
  is_buffer_loaded = function()
    error(loaded_api_secret)
  end,
  list_buffers = function()
    return { 1 }
  end,
}, DEFAULT_LIMITS, "keymap_snapshot:api_failed", loaded_api_secret)

local keytrans_secret = "keytrans-/private/secret"
assert_failure({
  get_global_keymaps = function(mode)
    return mode == "n" and { entry(keytrans_secret) } or {}
  end,
  keytrans = function()
    error(keytrans_secret)
  end,
}, DEFAULT_LIMITS, "keymap_snapshot:canonicalization_failed", keytrans_secret)

local deterministic = assert(collect({
  get_global_keymaps = function(mode)
    return mode == "n" and { entry("gg"), entry("a") } or {}
  end,
}))
local encoded_once = assert(snapshot.encode(deterministic, { max_encoded_bytes = DEFAULT_LIMITS.max_encoded_bytes }))
local encoded_twice = assert(snapshot.encode(copy(deterministic), { max_encoded_bytes = DEFAULT_LIMITS.max_encoded_bytes }))
assert(encoded_once == encoded_twice)
assert(string.sub(encoded_once, -1) == "\n")
assert(encoded_once == string.format(
  '{"snapshot_version":1,"mappings":[{"mapping_id":"%s","mode":"normal","scope":"global","lhs":["a"]},{"mapping_id":"%s","mode":"normal","scope":"global","lhs":["g","g"]}]}\n',
  real_id("normal", "global", { "a" }),
  real_id("normal", "global", { "g", "g" })
))

local hostile_encoded, hostile_encode_error = snapshot.encode({
  snapshot_version = 1,
  mappings = {
    {
      lhs = { "g" },
      mapping_id = "mapping-v1:/private/secret",
      mode = "normal",
      scope = "global",
    },
  },
}, { max_encoded_bytes = DEFAULT_LIMITS.max_encoded_bytes })
assert(hostile_encoded == nil and hostile_encode_error == "keymap_snapshot:invalid_snapshot")

-- Exercise one real API dictionary so changes to lhsraw/keytrans behavior do not
-- silently invalidate the pure adapter contract on supported Neovim versions.
local real_lhs = "<Space>k"
vim.keymap.set("n", real_lhs, "j", { desc = "key-insights-s2-canonicalization" })
local real_entry = nil
for _, candidate in ipairs(vim.api.nvim_get_keymap("n")) do
  if candidate.desc == "key-insights-s2-canonicalization" then
    real_entry = candidate
    break
  end
end
local real_ok, real_error = xpcall(function()
  assert(real_entry ~= nil)
  local tokens = assert(snapshot.canonicalize_lhs(real_entry, { keytrans = vim.fn.keytrans }, DEFAULT_LIMITS))
  assert(vim.deep_equal(tokens, { "<Space>", "k" }), vim.inspect(tokens))
end, debug.traceback)
vim.keymap.del("n", real_lhs)
assert(real_ok, real_error)

print("Lua keymap snapshot contract: ok")
