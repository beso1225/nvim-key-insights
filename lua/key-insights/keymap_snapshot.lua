local config = require("key-insights.config")
local key_tokens = require("key-insights.key_tokens")

local M = {}

M.VERSION = 1
local DEFAULT_LIMITS = {
  max_api_entries = 4096,
  max_buffers = 256,
  max_encoded_bytes = 1024 * 1024,
  max_lhs_bytes = 4096,
  max_lhs_tokens = 64,
  max_token_bytes = 256,
}

local QUERY_MODES = {
  { api = "n", normalized = "normal" },
  { api = "x", normalized = "visual" },
  { api = "o", normalized = "operator_pending" },
}

local VALID_MODES = {
  normal = true,
  operator_pending = true,
  visual = true,
}

local VALID_SCOPES = {
  buffer = true,
  global = true,
}

local function buffer_metadata(buffer)
  return {
    buftype = vim.bo[buffer].buftype,
    filetype = vim.bo[buffer].filetype,
    name = vim.api.nvim_buf_get_name(buffer),
  }
end

local DEFAULT_DEPENDENCIES = {
  get_buffer_keymaps = vim.api.nvim_buf_get_keymap,
  get_global_keymaps = vim.api.nvim_get_keymap,
  is_buffer_excluded = function(buffer, options)
    return config.is_excluded_buffer(buffer_metadata(buffer), options)
  end,
  is_buffer_loaded = vim.api.nvim_buf_is_loaded,
  is_buffer_sensitive = function(buffer)
    return config.is_sensitive_buffer(buffer_metadata(buffer))
  end,
  is_buffer_valid = vim.api.nvim_buf_is_valid,
  keytrans = vim.fn.keytrans,
  list_buffers = vim.api.nvim_list_bufs,
  sha256 = vim.fn.sha256,
}

local function resolved_limits(overrides)
  local limits = vim.tbl_extend("force", DEFAULT_LIMITS, overrides or {})
  for _, value in pairs(limits) do
    if type(value) ~= "number" or value < 1 or value >= math.huge or value ~= math.floor(value) then
      return nil
    end
  end
  return limits
end

local function safe_call(callback, ...)
  local result = { pcall(callback, ...) }
  if result[1] ~= true then
    return nil
  end
  return unpack(result, 2)
end

local function bounded_array(value, maximum)
  if type(value) ~= "table" then
    return nil, "invalid"
  end
  local count = 0
  local maximum_index = 0
  local index = nil
  while true do
    index = next(value, index)
    if index == nil then
      break
    end
    count = count + 1
    if count > maximum then
      return nil, "limit"
    end
    if type(index) ~= "number" or index < 1 or index ~= math.floor(index) then
      return nil, "invalid"
    end
    maximum_index = math.max(maximum_index, index)
  end
  if maximum_index ~= count then
    return nil, "invalid"
  end
  return count
end

local function length_prefix(value)
  return tostring(#value) .. ":" .. value
end

local function identity_preimage(mode, scope, tokens)
  local parts = {
    length_prefix("mapping-v1"),
    length_prefix(mode),
    length_prefix(scope),
    length_prefix(tostring(#tokens)),
  }
  for _, token in ipairs(tokens) do
    table.insert(parts, length_prefix(token))
  end
  return table.concat(parts)
end

local function is_continuation(byte)
  return byte ~= nil and byte >= 0x80 and byte <= 0xBF
end

local function valid_utf8(value)
  local index = 1
  while index <= #value do
    local first = string.byte(value, index)
    if first <= 0x7F then
      index = index + 1
    elseif first >= 0xC2 and first <= 0xDF then
      if not is_continuation(string.byte(value, index + 1)) then
        return false
      end
      index = index + 2
    elseif first >= 0xE0 and first <= 0xEF then
      local second = string.byte(value, index + 1)
      local third = string.byte(value, index + 2)
      local second_valid = is_continuation(second)
      if first == 0xE0 then
        second_valid = second ~= nil and second >= 0xA0 and second <= 0xBF
      elseif first == 0xED then
        second_valid = second ~= nil and second >= 0x80 and second <= 0x9F
      end
      if not second_valid or not is_continuation(third) then
        return false
      end
      index = index + 3
    elseif first >= 0xF0 and first <= 0xF4 then
      local second = string.byte(value, index + 1)
      local third = string.byte(value, index + 2)
      local fourth = string.byte(value, index + 3)
      local second_valid = is_continuation(second)
      if first == 0xF0 then
        second_valid = second ~= nil and second >= 0x90 and second <= 0xBF
      elseif first == 0xF4 then
        second_valid = second ~= nil and second >= 0x80 and second <= 0x8F
      end
      if not second_valid or not is_continuation(third) or not is_continuation(fourth) then
        return false
      end
      index = index + 4
    else
      return false
    end
  end
  return true
end

local function validate_tokens(tokens, limits)
  local count, array_error = bounded_array(tokens, limits.max_lhs_tokens)
  if count == nil then
    return nil, array_error == "limit" and "keymap_snapshot:limit_exceeded"
      or "keymap_snapshot:invalid_mapping"
  end
  if count == 0 then
    return nil, "keymap_snapshot:invalid_mapping"
  end
  local copy = {}
  local total_bytes = 0
  for index = 1, count do
    local token = tokens[index]
    if type(token) ~= "string" or token == "" then
      return nil, "keymap_snapshot:invalid_mapping"
    end
    if #token > limits.max_token_bytes then
      return nil, "keymap_snapshot:limit_exceeded"
    end
    total_bytes = total_bytes + #token
    if total_bytes > limits.max_lhs_bytes then
      return nil, "keymap_snapshot:limit_exceeded"
    end
    if string.find(token, "[%z\1-\31\127]") ~= nil then
      return nil, "keymap_snapshot:invalid_mapping"
    end
    if not valid_utf8(token) then
      return nil, "keymap_snapshot:invalid_mapping"
    end
    copy[index] = token
  end
  local reparsed = key_tokens.tokenize(table.concat(copy), {
    max_token_bytes = limits.max_token_bytes,
    max_tokens = limits.max_lhs_tokens,
  })
  if reparsed == nil or not vim.deep_equal(reparsed, copy) then
    return nil, "keymap_snapshot:invalid_mapping"
  end
  return copy
end

function M.canonicalize_lhs(mapping, dependencies, limit_overrides)
  local limits = resolved_limits(limit_overrides)
  if limits == nil or type(mapping) ~= "table" then
    return nil, "keymap_snapshot:invalid_mapping"
  end
  local lhsraw = rawget(mapping, "lhsraw")
  if type(lhsraw) ~= "string" or lhsraw == "" then
    return nil, "keymap_snapshot:invalid_mapping"
  end
  if #lhsraw > limits.max_lhs_bytes then
    return nil, "keymap_snapshot:limit_exceeded"
  end

  local keytrans = dependencies and dependencies.keytrans or DEFAULT_DEPENDENCIES.keytrans
  local canonical = safe_call(keytrans, lhsraw)
  if type(canonical) ~= "string" or canonical == "" then
    return nil, "keymap_snapshot:canonicalization_failed"
  end
  if #canonical > limits.max_lhs_bytes then
    return nil, "keymap_snapshot:limit_exceeded"
  end

  local tokens, token_error = key_tokens.tokenize(canonical, {
    max_token_bytes = limits.max_token_bytes,
    max_tokens = limits.max_lhs_tokens,
  })
  if tokens == nil then
    if token_error == "key_tokens:limit_exceeded" then
      return nil, "keymap_snapshot:limit_exceeded"
    end
    return nil, "keymap_snapshot:canonicalization_failed"
  end
  return validate_tokens(tokens, limits)
end

function M.mapping_id(mode, scope, tokens, dependencies, limit_overrides)
  local limits = resolved_limits(limit_overrides)
  if limits == nil or VALID_MODES[mode] ~= true or VALID_SCOPES[scope] ~= true then
    return nil, "keymap_snapshot:invalid_mapping"
  end
  local validated, validation_error = validate_tokens(tokens, limits)
  if validated == nil then
    return nil, validation_error
  end

  local sha256 = dependencies and dependencies.sha256 or DEFAULT_DEPENDENCIES.sha256
  local digest = safe_call(sha256, identity_preimage(mode, scope, validated))
  if type(digest) ~= "string" or string.match(digest, "^[0-9a-f][0-9a-f]+$") == nil or #digest ~= 64 then
    return nil, "keymap_snapshot:hash_failed"
  end
  return "mapping-v1:" .. digest
end

local function tuple_key(mode, scope, tokens)
  return identity_preimage(mode, scope, tokens)
end

local function tokens_less(left, right)
  local shared = math.min(#left, #right)
  for index = 1, shared do
    if left[index] ~= right[index] then
      return left[index] < right[index]
    end
  end
  return #left < #right
end

local function mapping_less(left, right)
  if left.mode ~= right.mode then
    return left.mode < right.mode
  end
  if not vim.deep_equal(left.lhs, right.lhs) then
    return tokens_less(left.lhs, right.lhs)
  end
  if left.scope ~= right.scope then
    return left.scope < right.scope
  end
  return left.mapping_id < right.mapping_id
end

local function json_string(value)
  local encoded = safe_call(vim.json.encode, value)
  if type(encoded) ~= "string" then
    return nil
  end
  return encoded
end

function M.encode(model, options, dependency_overrides)
  local limits = resolved_limits(options)
  if limits == nil or type(model) ~= "table" or rawget(model, "snapshot_version") ~= M.VERSION then
    return nil, "keymap_snapshot:invalid_snapshot"
  end
  local input_mappings = rawget(model, "mappings")
  local count, array_error = bounded_array(input_mappings, limits.max_api_entries)
  if count == nil then
    return nil, array_error == "limit" and "keymap_snapshot:limit_exceeded"
      or "keymap_snapshot:invalid_snapshot"
  end

  local dependencies = vim.tbl_extend("force", DEFAULT_DEPENDENCIES, dependency_overrides or {})
  local identities = {}
  local mappings = {}
  local tuples = {}
  for index = 1, count do
    local mapping = rawget(input_mappings, index)
    if type(mapping) ~= "table" then
      return nil, "keymap_snapshot:invalid_snapshot"
    end
    local mapping_id = rawget(mapping, "mapping_id")
    local mode = rawget(mapping, "mode")
    local scope = rawget(mapping, "scope")
    if type(mapping_id) ~= "string"
      or #mapping_id ~= 75
      or string.match(mapping_id, "^mapping%-v1:[0-9a-f]+$") == nil
      or VALID_MODES[mode] ~= true
      or VALID_SCOPES[scope] ~= true
    then
      return nil, "keymap_snapshot:invalid_snapshot"
    end
    local tokens, validation_error = validate_tokens(rawget(mapping, "lhs"), limits)
    if tokens == nil then
      return nil, validation_error
    end
    local expected_id, identity_error = M.mapping_id(mode, scope, tokens, dependencies, limits)
    if expected_id == nil then
      if identity_error == "keymap_snapshot:limit_exceeded" then
        return nil, identity_error
      end
      return nil, "keymap_snapshot:invalid_snapshot"
    end
    if mapping_id ~= expected_id then
      return nil, "keymap_snapshot:invalid_snapshot"
    end
    local key = tuple_key(mode, scope, tokens)
    if tuples[key] ~= nil or (identities[mapping_id] ~= nil and identities[mapping_id] ~= key) then
      return nil, "keymap_snapshot:invalid_snapshot"
    end
    tuples[key] = true
    identities[mapping_id] = key
    table.insert(mappings, {
      lhs = tokens,
      mapping_id = mapping_id,
      mode = mode,
      scope = scope,
    })
  end
  table.sort(mappings, mapping_less)

  local parts = { '{"snapshot_version":1,"mappings":[' }
  for index = 1, count do
    local mapping = mappings[index]
    local encoded_id = json_string(mapping.mapping_id)
    local encoded_mode = json_string(mapping.mode)
    local encoded_scope = json_string(mapping.scope)
    local encoded_lhs = json_string(mapping.lhs)
    if encoded_id == nil or encoded_mode == nil or encoded_scope == nil or encoded_lhs == nil then
      return nil, "keymap_snapshot:encoding_failed"
    end
    if index > 1 then
      table.insert(parts, ",")
    end
    table.insert(parts, '{"mapping_id":')
    table.insert(parts, encoded_id)
    table.insert(parts, ',"mode":')
    table.insert(parts, encoded_mode)
    table.insert(parts, ',"scope":')
    table.insert(parts, encoded_scope)
    table.insert(parts, ',"lhs":')
    table.insert(parts, encoded_lhs)
    table.insert(parts, "}")
  end
  table.insert(parts, "]}\n")
  local encoded = table.concat(parts)
  if #encoded > limits.max_encoded_bytes then
    return nil, "keymap_snapshot:limit_exceeded"
  end
  return encoded
end

function M.collect(options, dependency_overrides)
  local settings = options or {}
  local limits = resolved_limits(settings.limits)
  if limits == nil then
    return nil, "keymap_snapshot:invalid_limits"
  end
  local dependencies = vim.tbl_extend("force", DEFAULT_DEPENDENCIES, dependency_overrides or {})
  local mappings = {}
  local tuples = {}
  local identities = {}
  local api_entries = 0

  local function add_entries(entries, mode, scope)
    local remaining = limits.max_api_entries - api_entries
    local count, array_error = bounded_array(entries, remaining)
    if count == nil then
      return nil, array_error == "limit" and "keymap_snapshot:limit_exceeded"
        or "keymap_snapshot:api_failed"
    end
    api_entries = api_entries + count
    for index = 1, count do
      local tokens, canonicalization_error = M.canonicalize_lhs(rawget(entries, index), dependencies, limits)
      if tokens == nil then
        return nil, canonicalization_error
      end
      local mapping_id, identity_error = M.mapping_id(mode, scope, tokens, dependencies, limits)
      if mapping_id == nil then
        return nil, identity_error
      end
      local key = tuple_key(mode, scope, tokens)
      local previous_tuple = identities[mapping_id]
      if previous_tuple ~= nil and previous_tuple ~= key then
        return nil, "keymap_snapshot:identity_conflict"
      end
      identities[mapping_id] = key
      if tuples[key] == nil then
        tuples[key] = true
        table.insert(mappings, {
          mapping_id = mapping_id,
          mode = mode,
          scope = scope,
          lhs = tokens,
        })
      end
    end
    return true
  end

  for _, mode in ipairs(QUERY_MODES) do
    local entries = safe_call(dependencies.get_global_keymaps, mode.api)
    if entries == nil then
      return nil, "keymap_snapshot:api_failed"
    end
    local ok, error_code = add_entries(entries, mode.normalized, "global")
    if ok == nil then
      return nil, error_code
    end
  end

  local buffers = safe_call(dependencies.list_buffers)
  if buffers == nil then
    return nil, "keymap_snapshot:api_failed"
  end
  local buffer_count, buffer_error = bounded_array(buffers, limits.max_buffers)
  if buffer_count == nil then
    return nil, buffer_error == "limit" and "keymap_snapshot:limit_exceeded"
      or "keymap_snapshot:api_failed"
  end
  for index = 1, buffer_count do
    local buffer = rawget(buffers, index)
    local valid = safe_call(dependencies.is_buffer_valid, buffer)
    if type(valid) ~= "boolean" then
      return nil, "keymap_snapshot:api_failed"
    end
    local loaded = false
    if valid == true then
      loaded = safe_call(dependencies.is_buffer_loaded, buffer)
      if type(loaded) ~= "boolean" then
        return nil, "keymap_snapshot:api_failed"
      end
    end
    local excluded = true
    if loaded == true then
      excluded = safe_call(dependencies.is_buffer_excluded, buffer, settings.options)
      if type(excluded) ~= "boolean" then
        return nil, "keymap_snapshot:api_failed"
      end
    end
    local sensitive = true
    if excluded == false then
      sensitive = safe_call(dependencies.is_buffer_sensitive, buffer)
      if type(sensitive) ~= "boolean" then
        return nil, "keymap_snapshot:api_failed"
      end
    end
    if valid == true and loaded == true and excluded == false and sensitive == false then
      for _, mode in ipairs(QUERY_MODES) do
        local entries = safe_call(dependencies.get_buffer_keymaps, buffer, mode.api)
        if entries == nil then
          return nil, "keymap_snapshot:api_failed"
        end
        local ok, error_code = add_entries(entries, mode.normalized, "buffer")
        if ok == nil then
          return nil, error_code
        end
      end
    end
  end

  table.sort(mappings, mapping_less)
  local model = {
    snapshot_version = M.VERSION,
    mappings = mappings,
  }
  local encoded, encoding_error = M.encode(model, limits, dependencies)
  if encoded == nil then
    return nil, encoding_error
  end
  return model
end

return M
