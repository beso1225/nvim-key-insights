local M = {}

M.MAX_CALLBACK_BYTES = 4096
M.MAX_LHS_KEYS = 64
M.MAX_KEY_BYTES = 256

local ATTRIBUTABLE_EVIDENCE = {
  typed_different = true,
  typed_same = true,
}

local MODES = {
  normal = true,
  operator_pending = true,
  visual = true,
}

local SCOPES = {
  buffer = true,
  global = true,
}

function M.classify_callback(mapped, typed)
  if type(mapped) ~= "string" or type(typed) ~= "string" then
    return "unsupported"
  end
  if #mapped > M.MAX_CALLBACK_BYTES or #typed > M.MAX_CALLBACK_BYTES then
    return "unsupported"
  end
  if typed == "" then
    return "untyped"
  end
  if mapped == "" then
    return "typed_without_output"
  end
  if mapped == typed then
    return "typed_same"
  end
  return "typed_different"
end

local function one_candidate(candidates)
  if type(candidates) ~= "table" then
    return nil
  end
  local key, candidate = next(candidates)
  if key ~= 1 or next(candidates, key) ~= nil then
    return nil
  end
  return candidate
end

local function copy_lhs(lhs)
  if type(lhs) ~= "table" then
    return nil
  end

  local count = 0
  local maximum_index = 0
  for index, key in pairs(lhs) do
    count = count + 1
    if count > M.MAX_LHS_KEYS then
      return nil
    end
    if type(index) ~= "number" or index < 1 or index ~= math.floor(index) then
      return nil
    end
    if type(key) ~= "string" or key == "" or #key > M.MAX_KEY_BYTES then
      return nil
    end
    maximum_index = math.max(maximum_index, index)
  end
  if count == 0 or maximum_index ~= count then
    return nil
  end

  local copy = {}
  for index = 1, count do
    copy[index] = lhs[index]
  end
  return copy
end

function M.confirm(candidates, evidence)
  if ATTRIBUTABLE_EVIDENCE[evidence] ~= true then
    return nil
  end

  local candidate = one_candidate(candidates)
  if type(candidate) ~= "table" or candidate.exact ~= true or candidate.stable ~= true then
    return nil
  end
  if MODES[candidate.mode] ~= true or SCOPES[candidate.scope] ~= true then
    return nil
  end

  local lhs = copy_lhs(candidate.lhs)
  if lhs == nil then
    return nil
  end
  return {
    lhs = lhs,
    mode = candidate.mode,
    scope = candidate.scope,
  }
end

return M
