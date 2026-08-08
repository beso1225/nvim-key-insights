local M = {}

local MAX_KEY_NOTATION_BYTES = 256

local function valid_limit(value)
  return value == nil
    or (type(value) == "number" and value >= 0 and value < math.huge and value == math.floor(value))
end

local function is_continuation(byte)
  return byte ~= nil and byte >= 0x80 and byte <= 0xBF
end

local function character_bytes(value, index)
  local first = string.byte(value, index)
  if first == nil then
    return nil
  end
  if first <= 0x7F then
    return 1
  end
  if first >= 0xC2 and first <= 0xDF and is_continuation(string.byte(value, index + 1)) then
    return 2
  end
  if first >= 0xE0 and first <= 0xEF then
    local second = string.byte(value, index + 1)
    local third = string.byte(value, index + 2)
    local second_valid = is_continuation(second)
    if first == 0xE0 then
      second_valid = second ~= nil and second >= 0xA0 and second <= 0xBF
    elseif first == 0xED then
      second_valid = second ~= nil and second >= 0x80 and second <= 0x9F
    end
    return second_valid and is_continuation(third) and 3 or nil
  end
  if first >= 0xF0 and first <= 0xF4 then
    local second = string.byte(value, index + 1)
    local third = string.byte(value, index + 2)
    local fourth = string.byte(value, index + 3)
    local second_valid = is_continuation(second)
    if first == 0xF0 then
      second_valid = second ~= nil and second >= 0x90 and second <= 0xBF
    elseif first == 0xF4 then
      second_valid = second ~= nil and second >= 0x80 and second <= 0x8F
    end
    return second_valid and is_continuation(third) and is_continuation(fourth) and 4 or nil
  end
  return nil
end

local function valid_utf8(value)
  local index = 1
  while index <= #value do
    local width = character_bytes(value, index)
    if width == nil then
      return false
    end
    index = index + width
  end
  return true
end

function M.tokenize(canonical, limits)
  if type(canonical) ~= "string" then
    return nil, "key_tokens:invalid_input"
  end
  limits = limits or {}
  local max_input_bytes = type(limits) == "table" and rawget(limits, "max_input_bytes") or nil
  local max_tokens = type(limits) == "table" and rawget(limits, "max_tokens") or nil
  local max_token_bytes = type(limits) == "table" and rawget(limits, "max_token_bytes") or nil
  if type(limits) ~= "table"
    or not valid_limit(max_input_bytes)
    or not valid_limit(max_tokens)
    or not valid_limit(max_token_bytes)
  then
    return nil, "key_tokens:invalid_limits"
  end
  if canonical == "" then
    return {}
  end
  if max_input_bytes ~= nil and #canonical > max_input_bytes then
    return nil, "key_tokens:limit_exceeded"
  end
  if not valid_utf8(canonical) then
    return nil, "key_tokens:invalid_input"
  end

  local tokens = {}
  local function append(token)
    if max_tokens ~= nil and #tokens >= max_tokens then
      return false
    end
    if max_token_bytes ~= nil and #token > max_token_bytes then
      return false
    end
    table.insert(tokens, token)
    return true
  end

  local index = 1
  local next_closing = nil
  local closing_search_from = 1
  local closing_search_exhausted = false

  local function closing_at_or_after(start)
    while next_closing ~= nil and next_closing < start do
      next_closing = nil
    end
    if next_closing == nil and not closing_search_exhausted then
      local search_from = math.max(start, closing_search_from)
      next_closing = string.find(canonical, ">", search_from, true)
      if next_closing == nil then
        closing_search_exhausted = true
      else
        closing_search_from = next_closing + 1
      end
    end
    return next_closing
  end

  while index <= #canonical do
    local width = character_bytes(canonical, index)
    local token = string.sub(canonical, index, index + width - 1)
    local next_index = index + width
    if token == "<" then
      local closing = closing_at_or_after(next_index)
      if closing ~= nil and closing - index + 1 <= MAX_KEY_NOTATION_BYTES then
        token = string.sub(canonical, index, closing)
        next_index = closing + 1
      end
    end

    if not append(token) then
      return nil, "key_tokens:limit_exceeded"
    end
    index = next_index
  end

  return tokens
end

return M
