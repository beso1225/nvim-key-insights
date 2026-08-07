local snapshot = require("key-insights.keymap_snapshot")

local M = {}
local Resolver = {}
Resolver.__index = Resolver

local MODES = {
  normal = "n",
  operator_pending = "o",
  visual = "x",
}

local DEFAULT_LIMITS = {
  max_api_entries = 4096,
  max_lhs_bytes = 4096,
  max_lhs_tokens = 64,
  max_token_bytes = 256,
}

local DEFAULT_DEPENDENCIES = {
  get_buffer_keymaps = vim.api.nvim_buf_get_keymap,
  current_buffer = vim.api.nvim_get_current_buf,
  get_global_keymaps = vim.api.nvim_get_keymap,
  keytrans = vim.fn.keytrans,
  maparg = function(lhs, mode)
    return vim.fn.maparg(lhs, mode, false, true)
  end,
  sha256 = vim.fn.sha256,
}

local function resolve_table(defaults, overrides, require_functions)
  if overrides ~= nil and type(overrides) ~= "table" then
    return nil
  end
  local resolved = {}
  for name, default in pairs(defaults) do
    local value = nil
    if overrides ~= nil then
      value = rawget(overrides, name)
    end
    resolved[name] = value == nil and default or value
    if require_functions and type(resolved[name]) ~= "function" then
      return nil
    end
  end
  return resolved
end

local function resolve_limits(overrides)
  local limits = resolve_table(DEFAULT_LIMITS, overrides, false)
  if limits == nil then
    return nil
  end
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
    return nil
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
    if count > maximum or type(index) ~= "number" or index < 1 or index ~= math.floor(index) then
      return nil
    end
    maximum_index = math.max(maximum_index, index)
  end
  if maximum_index ~= count then
    return nil
  end
  return count
end

local function length_prefix(value)
  return tostring(#value) .. ":" .. value
end

local function token_key(tokens)
  local parts = { length_prefix(tostring(#tokens)) }
  for _, token in ipairs(tokens) do
    table.insert(parts, length_prefix(token))
  end
  return table.concat(parts)
end

local function project(mapping, mode, scope, dependencies, limits)
  local tokens, canonical_error = snapshot.canonicalize_lhs(mapping, {
    keytrans = dependencies.keytrans,
  }, limits)
  if tokens == nil then
    return nil, canonical_error
  end
  local mapping_id, identity_error = snapshot.mapping_id(mode, scope, tokens, {
    sha256 = dependencies.sha256,
  }, limits)
  if mapping_id == nil then
    return nil, identity_error
  end
  return {
    lhs = tokens,
    mapping_id = mapping_id,
    mode = mode,
    scope = scope,
  }
end

local function mark_prefix_ambiguity(entries)
  local root = { children = {} }
  for _, entry in pairs(entries) do
    local node = root
    for _, token in ipairs(entry.lhs) do
      local child = node.children[token]
      if child == nil then
        child = { children = {} }
        node.children[token] = child
      end
      node = child
    end
    node.entry = entry
  end

  local function visit(node, has_terminal_ancestor)
    local descendant_has_terminal = false
    local next_has_terminal_ancestor = has_terminal_ancestor or node.entry ~= nil
    for _, child in pairs(node.children) do
      if visit(child, next_has_terminal_ancestor) then
        descendant_has_terminal = true
      end
    end
    if node.entry ~= nil and (has_terminal_ancestor or descendant_has_terminal) then
      node.entry.ambiguous = true
    end
    return node.entry ~= nil or descendant_has_terminal
  end
  visit(root, false)
end

function M.new(spec)
  if spec ~= nil and type(spec) ~= "table" then
    return nil, "mapping_resolver:invalid_configuration"
  end
  local settings = spec or {}
  local dependency_overrides = rawget(settings, "dependencies")
  local limit_overrides = nil
  if dependency_overrides ~= nil then
    limit_overrides = rawget(settings, "limits")
  else
    dependency_overrides = settings
  end
  local dependencies = resolve_table(DEFAULT_DEPENDENCIES, dependency_overrides, true)
  local limits = resolve_limits(limit_overrides)
  if dependencies == nil or limits == nil then
    return nil, "mapping_resolver:invalid_configuration"
  end
  return setmetatable({
    _baseline = nil,
    _buffer = nil,
    _dependencies = dependencies,
    _dirty = false,
    _limits = limits,
  }, Resolver)
end

function Resolver:reset()
  self._baseline = nil
  self._buffer = nil
  self._dirty = false
end

function Resolver:is_dirty()
  return self._dirty
end

function Resolver:boundary()
  -- Resolution is callback-local. A boundary does not make a sanitized
  -- point-in-time baseline stale by itself.
end

function Resolver:prime(buffer)
  self:reset()
  if type(buffer) ~= "number" or buffer < 1 or buffer ~= math.floor(buffer) then
    return nil, "mapping_resolver:invalid_context"
  end

  local baseline = {}
  local inspected = 0
  for mode, api_mode in pairs(MODES) do
    local globals = safe_call(self._dependencies.get_global_keymaps, api_mode)
    local locals = safe_call(self._dependencies.get_buffer_keymaps, buffer, api_mode)
    local global_count = bounded_array(globals, self._limits.max_api_entries - inspected)
    if global_count == nil then
      return nil, "mapping_resolver:api_failed"
    end
    inspected = inspected + global_count
    local local_count = bounded_array(locals, self._limits.max_api_entries - inspected)
    if local_count == nil then
      return nil, "mapping_resolver:api_failed"
    end
    inspected = inspected + local_count

    local effective = {}
    local function add(entries, count, scope)
      for index = 1, count do
        local candidate = project(rawget(entries, index), mode, scope, self._dependencies, self._limits)
        if candidate == nil then
          return nil
        end
        local key = token_key(candidate.lhs)
        if effective[key] ~= nil and effective[key].scope == scope then
          return nil
        end
        effective[key] = candidate
      end
      return true
    end
    if add(globals, global_count, "global") == nil or add(locals, local_count, "buffer") == nil then
      return nil, "mapping_resolver:invalid_mapping"
    end
    mark_prefix_ambiguity(effective)
    baseline[mode] = effective
  end

  self._baseline = baseline
  self._buffer = buffer
  return true
end

function Resolver:resolve(mode, typed_tokens)
  if self._baseline == nil or self._dirty or MODES[mode] == nil or type(typed_tokens) ~= "table" then
    return nil
  end
  local current_buffer = safe_call(self._dependencies.current_buffer)
  if current_buffer ~= self._buffer then
    self._dirty = true
    return nil
  end
  local candidate = self._baseline[mode][token_key(typed_tokens)]
  if candidate == nil or candidate.ambiguous == true then
    return nil
  end

  local live = safe_call(self._dependencies.maparg, table.concat(typed_tokens), MODES[mode])
  if type(live) ~= "table" then
    self._dirty = true
    return nil
  end
  local scope_marker = rawget(live, "buffer")
  if scope_marker ~= 0 and scope_marker ~= 1 then
    self._dirty = true
    return nil
  end
  local scope = scope_marker == 1 and "buffer" or "global"
  local projected = project(live, mode, scope, self._dependencies, self._limits)
  local buffer_after = safe_call(self._dependencies.current_buffer)
  if projected == nil
    or buffer_after ~= self._buffer
    or projected.mapping_id ~= candidate.mapping_id
    or projected.scope ~= candidate.scope
  then
    self._dirty = true
    return nil
  end
  return {
    mapping_id = candidate.mapping_id,
    mode = candidate.mode,
    scope = candidate.scope,
  }
end

return M
