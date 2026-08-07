local M = {}

local MAX_KEY_NOTATION_BYTES = 256

local function valid_limit(value)
  return value == nil
    or (type(value) == "number" and value >= 0 and value < math.huge and value == math.floor(value))
end

function M.tokenize(canonical, limits)
  if type(canonical) ~= "string" then
    return nil, "key_tokens:invalid_input"
  end
  limits = limits or {}
  if type(limits) ~= "table"
    or not valid_limit(limits.max_tokens)
    or not valid_limit(limits.max_token_bytes)
  then
    return nil, "key_tokens:invalid_limits"
  end
  if canonical == "" then
    return {}
  end

  local ok, characters = pcall(vim.fn.split, canonical, "\\zs")
  if not ok or type(characters) ~= "table" then
    return nil, "key_tokens:invalid_input"
  end

  local tokens = {}
  local function append(token)
    if limits.max_tokens ~= nil and #tokens >= limits.max_tokens then
      return false
    end
    if limits.max_token_bytes ~= nil and #token > limits.max_token_bytes then
      return false
    end
    table.insert(tokens, token)
    return true
  end

  local index = 1
  while index <= #characters do
    local token = characters[index]
    local next_index = index + 1
    if token == "<" then
      local closing = index + 1
      while closing <= #characters and characters[closing] ~= ">" do
        closing = closing + 1
      end
      local notation = closing <= #characters and table.concat(characters, "", index, closing) or nil
      if notation ~= nil and #notation <= MAX_KEY_NOTATION_BYTES then
        token = notation
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
