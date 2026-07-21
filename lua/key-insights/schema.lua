local M = {}

M.VERSION = 1

local sequence_modes = {
  normal = true,
  visual = true,
  operator_pending = true,
}

local modes = vim.tbl_extend("force", sequence_modes, {
  insert = true,
  command = true,
  search = true,
  other = true,
})

local function assert_nonempty_string(value, name)
  assert(type(value) == "string" and value ~= "", name .. " must be a non-empty string")
end

local function assert_nonnegative_integer(value, name)
  assert(type(value) == "number" and value >= 0 and value == math.floor(value), name .. " must be a non-negative integer")
end

local function envelope(event_type, session_id, elapsed_ms)
  assert_nonempty_string(session_id, "session_id")
  assert_nonnegative_integer(elapsed_ms, "elapsed_ms")

  return {
    schema_version = M.VERSION,
    event_type = event_type,
    session_id = session_id,
    elapsed_ms = elapsed_ms,
  }
end

local function copy_keys(keys, name)
  assert(type(keys) == "table" and #keys > 0, name .. " must be a non-empty list")

  local result = {}
  for index, key in ipairs(keys) do
    assert_nonempty_string(key, name .. "[" .. index .. "]")
    result[index] = key
  end
  return result
end

function M.session_start(session_id, project_id)
  local event = envelope("session_start", session_id, 0)
  if project_id ~= nil then
    assert_nonempty_string(project_id, "project_id")
    event.project_id = project_id
  end
  return event
end

function M.session_end(session_id, elapsed_ms)
  return envelope("session_end", session_id, elapsed_ms)
end

function M.key_sequence(session_id, elapsed_ms, mode, keys, duration_ms)
  assert(sequence_modes[mode] == true, "key_sequence mode must not contain text")
  assert_nonnegative_integer(duration_ms, "duration_ms")

  local event = envelope("key_sequence", session_id, elapsed_ms)
  event.mode = mode
  event.keys = copy_keys(keys, "keys")
  event.duration_ms = duration_ms
  return event
end

function M.text_run(session_id, elapsed_ms, key_count, duration_ms)
  assert_nonnegative_integer(key_count, "key_count")
  assert_nonnegative_integer(duration_ms, "duration_ms")

  local event = envelope("text_run", session_id, elapsed_ms)
  event.key_count = key_count
  event.duration_ms = duration_ms
  return event
end

function M.mode_transition(session_id, elapsed_ms, from, to)
  assert(modes[from] == true, "invalid source mode")
  assert(modes[to] == true, "invalid destination mode")

  local event = envelope("mode_transition", session_id, elapsed_ms)
  event.from = from
  event.to = to
  return event
end

function M.mapping_use(session_id, elapsed_ms, mode, mapping_id, typed_keys)
  assert(sequence_modes[mode] == true, "mapping mode must not contain text")
  assert_nonempty_string(mapping_id, "mapping_id")

  local event = envelope("mapping_use", session_id, elapsed_ms)
  event.mode = mode
  event.mapping_id = mapping_id
  event.typed_keys = copy_keys(typed_keys, "typed_keys")
  return event
end

function M.encode(event)
  assert(type(event) == "table", "event must be a table")
  return vim.json.encode(event) .. "\n"
end

return M
