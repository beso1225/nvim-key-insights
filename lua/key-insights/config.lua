local M = {}

local SENSITIVE_NAME_PATTERNS = {
  "^%.env",
  "credential",
  "secret",
  "%.pem$",
  "%.key$",
  "id_[er]sa",
}

local SENSITIVE_FILETYPES = {
  dotenv = true,
  sshconfig = true,
}

local DEFAULTS = {
  privacy = {
    raw_keylog = false,
    capture_insert_text = false,
    capture_command_text = false,
    capture_search_text = false,
    store_file_paths = false,
  },
  collection = {
    exclude_special_buffers = true,
  },
}

function M.defaults()
  return vim.deepcopy(DEFAULTS)
end

function M.is_excluded_buffer(buffer, options)
  local config = options or DEFAULTS
  return config.collection.exclude_special_buffers and buffer.buftype ~= ""
end

function M.is_sensitive_name(name, _options)
  local normalized = string.lower(name or "")
  local basename = vim.fs.basename(normalized)

  for _, pattern in ipairs(SENSITIVE_NAME_PATTERNS) do
    if string.find(basename, pattern) then
      return true
    end
  end

  return false
end

function M.is_sensitive_buffer(buffer)
  local filetype = string.lower(buffer.filetype or "")
  return SENSITIVE_FILETYPES[filetype] == true or M.is_sensitive_name(buffer.name)
end

return M
