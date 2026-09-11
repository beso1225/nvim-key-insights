local M = {}

local MAX_KEY_NOTATION_BYTES = 256

local NAMED_CONTROL_TOKENS = {
  ["<Nul>"] = true,
  ["<BS>"] = true,
  ["<Tab>"] = true,
  ["<NL>"] = true,
  ["<CR>"] = true,
  ["<Return>"] = true,
  ["<Enter>"] = true,
  ["<Esc>"] = true,
  ["<Space>"] = true,
  ["<Del>"] = true,
  ["<Delete>"] = true,
  ["<Insert>"] = true,
  ["<Home>"] = true,
  ["<End>"] = true,
  ["<PageUp>"] = true,
  ["<PageDown>"] = true,
  ["<Up>"] = true,
  ["<Down>"] = true,
  ["<Left>"] = true,
  ["<Right>"] = true,
  ["<kHome>"] = true,
  ["<kEnd>"] = true,
  ["<kPageUp>"] = true,
  ["<kPageDown>"] = true,
  ["<kUp>"] = true,
  ["<kDown>"] = true,
  ["<kLeft>"] = true,
  ["<kRight>"] = true,
}

local SAFE_BRACKETED_CONTROL_TOKENS = {
  ["<C-/>"] = true,
  ["<A-/>"] = true,
  ["<M-/>"] = true,
  ["<S-/>"] = true,
  ["<C-\\>"] = true,
  ["<A-\\>"] = true,
  ["<M-\\>"] = true,
  ["<S-\\>"] = true,
}

local CARET_MARKERS = {
  [27] = "[",
  [28] = "\\",
  [29] = "]",
  [30] = "^",
  [31] = "_",
  [127] = "?",
}

local function caret_marker(byte)
  if byte == 0 then
    return "@"
  end
  if byte >= 1 and byte <= 26 then
    return string.char(byte + 64)
  end
  return CARET_MARKERS[byte]
end

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

function M.normalize_caret_notation(canonical, typed)
  if type(canonical) ~= "string" or type(typed) ~= "string" or typed == "" then
    return canonical
  end

  -- Some Neovim versions expose C0 bytes from keytrans as caret pairs. Only
  -- normalize when the raw callback input also contains a control byte so
  -- literal text such as ^Y remains text.
  local has_control = false
  for index = 1, #typed do
    local byte = string.byte(typed, index)
    if byte < 0x20 or byte == 0x7F then
      has_control = true
      break
    end
  end
  if not has_control then
    return canonical
  end

  local caret_form = {}
  for index = 1, #typed do
    local byte = string.byte(typed, index)
    local marker = caret_marker(byte)
    if marker ~= nil then
      table.insert(caret_form, "^" .. marker)
    else
      table.insert(caret_form, string.sub(typed, index, index))
    end
  end
  if table.concat(caret_form) ~= canonical then
    return canonical
  end

  local normalized = {}
  for index = 1, #typed do
    local marker = caret_marker(string.byte(typed, index))
    if marker ~= nil then
      if marker == "[" then
        table.insert(normalized, "<Esc>")
      elseif marker == "?" then
        table.insert(normalized, "<Del>")
      else
        table.insert(normalized, "<C-" .. marker .. ">")
      end
    else
      table.insert(normalized, string.sub(typed, index, index))
    end
  end
  return table.concat(normalized)
end

local function chunk_end(value, start, max_bytes, next_closing)
  local limit = math.min(#value, start + max_bytes - 1)
  local cursor = start
  local end_index = start - 1
  while cursor <= limit do
    local width = character_bytes(value, cursor)
    if width == nil or cursor + width - 1 > limit then
      break
    end

    local next_end = cursor + width - 1
    if string.byte(value, cursor) == string.byte("<") then
      if next_closing ~= nil and next_closing <= cursor then
        next_closing = string.find(value, ">", cursor + width, true)
      end
      local closing = next_closing
      if closing ~= nil and closing - cursor + 1 <= MAX_KEY_NOTATION_BYTES then
        -- Preserve bracketed notation only when the whole token fits in this
        -- chunk. If the caller chooses a smaller byte bound, split it as
        -- ordinary UTF-8 text rather than exceeding the requested bound.
        if closing <= limit then
          next_end = closing
        elseif end_index >= start then
          break
        end
      end
    end

    end_index = next_end
    cursor = next_end + 1
  end
  return end_index, next_closing
end

function M.each_chunk(value, max_bytes, visitor)
  if type(value) ~= "string" then
    return nil, "key_tokens:invalid_input"
  end
  if type(max_bytes) ~= "number"
    or max_bytes <= 0
    or max_bytes == math.huge
    or max_bytes ~= math.floor(max_bytes)
  then
    return nil, "key_tokens:invalid_limits"
  end
  if type(visitor) ~= "function" then
    return nil, "key_tokens:invalid_visitor"
  end
  if value == "" then
    return true
  end
  if not valid_utf8(value) then
    return nil, "key_tokens:invalid_input"
  end

  local start = 1
  local next_closing = string.find(value, ">", 2, true)
  while start <= #value do
    local ending
    ending, next_closing = chunk_end(value, start, max_bytes, next_closing)
    if ending < start then
      return nil, "key_tokens:invalid_limits"
    end
    if visitor(string.sub(value, start, ending)) == false then
      return false
    end
    start = ending + 1
  end
  return true
end

function M.chunk(value, max_bytes)
  local chunks = {}
  local ok, error_code = M.each_chunk(value, max_bytes, function(chunk)
    table.insert(chunks, chunk)
    return true
  end)
  if not ok then
    return nil, error_code
  end
  return chunks
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

function M.is_control_token(token)
  if type(token) ~= "string" or token == "" or #token > MAX_KEY_NOTATION_BYTES then
    return false
  end
  if NAMED_CONTROL_TOKENS[token] then
    return true
  end
  if SAFE_BRACKETED_CONTROL_TOKENS[token] then
    return true
  end
  if string.match(token, "^<F%d+>$") ~= nil then
    return true
  end
  local key = string.match(token, "^<[CASMD]%-([^>]+)>$")
  if key == nil or key == "" then
    return false
  end
  local lower = string.lower(token)
  if string.find(lower, ".env", 1, true) ~= nil
    or string.find(lower, "secret", 1, true) ~= nil
    or string.find(lower, "credential", 1, true) ~= nil
  then
    return false
  end
  for index = 1, #key do
    local byte = string.byte(key, index)
    if byte < 0x20 or byte > 0x7E or byte == 0x2F or byte == 0x5C then
      return false
    end
  end
  return true
end

return M
