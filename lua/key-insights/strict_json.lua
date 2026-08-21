local M = {}

local DEFAULT_MAX_DEPTH = 128

local function skip_whitespace(contents, index)
  while index <= #contents and string.match(string.sub(contents, index, index), "%s") do
    index = index + 1
  end
  return index
end

local function scan_string(contents, index)
  if string.sub(contents, index, index) ~= '"' then
    return nil, nil
  end
  local start = index
  index = index + 1
  while index <= #contents do
    local byte = string.sub(contents, index, index)
    if byte == '"' then
      local literal = string.sub(contents, start, index)
      local decoded_ok, decoded = pcall(vim.json.decode, literal)
      if not decoded_ok or type(decoded) ~= "string" then
        return nil, nil
      end
      return decoded, index + 1
    end
    if byte == "\\" then
      index = index + 1
      if index > #contents then
        return nil, nil
      end
    end
    index = index + 1
  end
  return nil, nil
end

local scan_value

local function scan_object(contents, index, depth, maximum_depth)
  index = skip_whitespace(contents, index + 1)
  if string.sub(contents, index, index) == "}" then
    return false, index + 1
  end
  local keys = {}
  local duplicate = false
  while index <= #contents do
    local key
    key, index = scan_string(contents, index)
    if key == nil then
      return nil, nil
    end
    if keys[key] then
      duplicate = true
    end
    keys[key] = true
    index = skip_whitespace(contents, index)
    if string.sub(contents, index, index) ~= ":" then
      return nil, nil
    end
    local nested_duplicate
    nested_duplicate, index = scan_value(contents, index + 1, depth + 1, maximum_depth)
    if nested_duplicate == nil then
      return nil, nil
    end
    duplicate = duplicate or nested_duplicate
    index = skip_whitespace(contents, index)
    local separator = string.sub(contents, index, index)
    if separator == "}" then
      return duplicate, index + 1
    end
    if separator ~= "," then
      return nil, nil
    end
    index = skip_whitespace(contents, index + 1)
  end
  return nil, nil
end

local function scan_array(contents, index, depth, maximum_depth)
  index = skip_whitespace(contents, index + 1)
  if string.sub(contents, index, index) == "]" then
    return false, index + 1
  end
  local duplicate = false
  while index <= #contents do
    local nested_duplicate
    nested_duplicate, index = scan_value(contents, index, depth + 1, maximum_depth)
    if nested_duplicate == nil then
      return nil, nil
    end
    duplicate = duplicate or nested_duplicate
    index = skip_whitespace(contents, index)
    local separator = string.sub(contents, index, index)
    if separator == "]" then
      return duplicate, index + 1
    end
    if separator ~= "," then
      return nil, nil
    end
    index = skip_whitespace(contents, index + 1)
  end
  return nil, nil
end

scan_value = function(contents, index, depth, maximum_depth)
  if depth > maximum_depth then
    return nil, nil
  end
  index = skip_whitespace(contents, index)
  local byte = string.sub(contents, index, index)
  if byte == "{" then
    return scan_object(contents, index, depth, maximum_depth)
  end
  if byte == "[" then
    return scan_array(contents, index, depth, maximum_depth)
  end
  if byte == '"' then
    local _, next_index = scan_string(contents, index)
    if next_index == nil then
      return nil, nil
    end
    return false, next_index
  end
  for _, literal in ipairs({ "true", "false", "null" }) do
    if string.sub(contents, index, index + #literal - 1) == literal then
      return false, index + #literal
    end
  end
  local number_end = index
  while number_end <= #contents and string.match(string.sub(contents, number_end, number_end), "[0-9eE+%.%-]") do
    number_end = number_end + 1
  end
  if number_end == index then
    return nil, nil
  end
  return false, number_end
end

function M.decode(contents, maximum_depth)
  if type(contents) ~= "string" then
    return nil, "JSON input must be a string"
  end
  local call_ok, duplicate, index = pcall(scan_value, contents, 1, 0, maximum_depth or DEFAULT_MAX_DEPTH)
  if not call_ok or duplicate == nil or duplicate then
    return nil, duplicate and "JSON contains duplicate object keys" or "JSON is invalid"
  end
  if skip_whitespace(contents, index) ~= #contents + 1 then
    return nil, "JSON has trailing content"
  end
  local decoded_ok, decoded = pcall(vim.json.decode, contents)
  if not decoded_ok then
    return nil, "JSON is invalid"
  end
  return decoded
end

return M
