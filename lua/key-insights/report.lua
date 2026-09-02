local contract_versions = require("key-insights.contract_versions")
local filesystem = require("key-insights.filesystem")
local process = require("key-insights.process")
local snapshot_payload = require("key-insights.snapshot_payload")
local strict_json = require("key-insights.strict_json")

local M = {}
local Report = {}
Report.__index = Report

local OWNER_DIRECTORY = 448 -- 0700
local MAX_SUMMARY_BYTES = 16 * 1024 * 1024
local MAX_REPORT_BYTES = 16 * 1024 * 1024
local MAX_PREVIEW_BYTES = 256 * 1024
local REPORT_HEADER = "# Neovim Key Insights"
local PREVIEW_PURPOSE = "analyze-neovim-usage"
local PREVIEW_PRIVACY_BOUNDARY = "Use only aggregate evidence and the optional sanitized keymap snapshot; do not request or infer raw input."
local PREVIEW_ACTION_KINDS = { "learn_existing", "add_mapping", "change_mapping", "no_change" }
local MAX_CODEX_BYTES = 256 * 1024
local MAX_RENDERED_SUGGESTIONS_BYTES = 1024 * 1024
local MAX_CODEX_SUGGESTIONS = 100
local MAX_SUGGESTION_EVIDENCE = 32
local MAX_SUGGESTION_CONFLICTS = 4096
local MAX_EXACT_JSON_INTEGER = 9007199254740991

local FORBIDDEN_PREVIEW_KEYS = {
  command = true,
  file_path = true,
  implementation = true,
  path = true,
  project_id = true,
  raw_log = true,
  report = true,
  rhs = true,
  search = true,
  secret = true,
  session_id = true,
}

local KNOWN_PREVIEW_KEYS = {
  action_kinds = true,
  average_inter_key_latency_ms = true,
  buffer_mapping_id = true,
  bucket = true,
  candidate_id = true,
  candidate_limit = true,
  candidates = true,
  collision_mapping_ids = true,
  collision_check_required = true,
  collisions = true,
  contract_version = true,
  count = true,
  count_prefixes = true,
  digit_presses = true,
  distributions = true,
  ergonomics = true,
  events = true,
  evidence_required = true,
  from = true,
  global_mapping_id = true,
  guard = true,
  histogram_version = true,
  instructions = true,
  items = true,
  key = true,
  keymap_snapshot = true,
  key_sequences = true,
  keys = true,
  kind = true,
  kind_version = true,
  lhs = true,
  mapping_attribution = true,
  mapping_coverage = true,
  mapping_id = true,
  mapping_uses = true,
  mappings = true,
  measurements = true,
  minimum_candidate_observations = true,
  minimum_candidate_sequence_keys = true,
  minimum_candidate_sessions = true,
  mode = true,
  mode_transitions = true,
  modes = true,
  motion = true,
  observed_mappings = true,
  observed_sessions = true,
  observed_sequence_keys = true,
  observed_uses = true,
  observations = true,
  occurrences = true,
  operations = true,
  payload_schema_version = true,
  presses = true,
  privacy_boundary = true,
  project_id = true,
  purpose = true,
  ranking_limit = true,
  redo = true,
  ["repeat"] = true,
  repeated_key_presses = true,
  repeated_key_runs = true,
  repeated_keys = true,
  repeated_motions = true,
  required_observations = true,
  required_sequence_keys = true,
  required_sessions = true,
  runs = true,
  sampled_sessions = true,
  schema_version = true,
  search_navigation = true,
  search_start = true,
  sequences = true,
  session_duration_ms = true,
  sequence_keys = true,
  sequence_length_keys = true,
  sessions = true,
  scope = true,
  snapshot_version = true,
  summary = true,
  status = true,
  text_keys = true,
  text_runs = true,
  thresholds = true,
  to = true,
  token_set_version = true,
  total_session_duration_ms = true,
  total_snapshot_mappings = true,
  undo = true,
  unique_keys = true,
  unique_mappings = true,
  unique_repeated_keys = true,
  unobserved_mappings = true,
}

local function default_notify(message, level)
  vim.notify("key-insights: " .. message, level)
end

local function default_open_file(path)
  vim.api.nvim_cmd({ cmd = "edit", args = { path } }, {})
end

local function default_open_contents(contents, filetype)
  local buffer = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_buf_set_lines(buffer, 0, -1, false, vim.split(contents, "\n", { plain = true }))
  vim.bo[buffer].buftype = "nofile"
  vim.bo[buffer].bufhidden = "wipe"
  vim.bo[buffer].swapfile = false
  vim.bo[buffer].filetype = filetype
  vim.bo[buffer].modifiable = false
  vim.api.nvim_set_current_buf(buffer)
end

local function default_open_preview(contents)
  default_open_contents(contents, "json")
end

local function default_open_suggestions(contents)
  default_open_contents(contents, "markdown")
end

local function default_confirm(callback, codex_binary)
  vim.ui.select({ "Run Codex analysis", "Cancel" }, {
    prompt = "Run " .. vim.inspect(codex_binary) .. " with this sanitized payload?",
  }, function(choice)
    callback(choice == "Run Codex analysis")
  end)
end

local function default_codex_schema()
  local source = debug.getinfo(1, "S").source
  local path = source:sub(1, 1) == "@" and source:sub(2) or source
  return vim.fs.joinpath(vim.fs.dirname(path), "..", "..", "codex", "suggestions.schema.json")
end

local validate_analysis_summary
local is_array
local expected_snapshot_mapping_id
local safe_preview_token
local validate_snapshot_context

local function validate_preview(contents)
  if type(contents) ~= "string" or #contents > MAX_PREVIEW_BYTES then
    return false, "preview output exceeds the size limit"
  end
  local decoded = strict_json.decode(contents)
  if type(decoded) ~= "table" then
    return false, "preview output has an unexpected format"
  end
  local top_level = {
    instructions = true,
    keymap_snapshot = true,
    payload_schema_version = true,
    purpose = true,
    summary = true,
  }
  for key in pairs(decoded) do
    if type(key) ~= "string" or not top_level[key] then
      return false, "preview output has an unexpected field"
    end
  end
  if decoded.payload_schema_version ~= contract_versions.codex_payload
    or decoded.purpose ~= PREVIEW_PURPOSE
    or not validate_analysis_summary(decoded.summary)
  then
    return false, "preview output has an unexpected format"
  end
  if (decoded.keymap_snapshot == nil) ~= (decoded.summary.mapping_attribution == nil) then
    return false, "preview output has an unexpected mapping attribution"
  end
  if not validate_snapshot_context(decoded.summary, decoded.keymap_snapshot) then
    return false, "preview output has an invalid keymap snapshot"
  end
  if decoded.keymap_snapshot ~= nil then
    local attribution = decoded.summary.mapping_attribution
    if type(decoded.keymap_snapshot) ~= "table"
      or decoded.keymap_snapshot.snapshot_version ~= contract_versions.keymap_snapshot
      or not is_array(decoded.keymap_snapshot.mappings, 4096)
      or type(attribution) ~= "table"
      or attribution.snapshot_version ~= decoded.keymap_snapshot.snapshot_version
      or not is_array(attribution.mappings, 4096)
      or not is_array(attribution.collisions, 4096)
    then
      return false, "preview output has an unexpected mapping attribution"
    end
  end
  local summary_fields = {
    ergonomics = true,
    events = true,
    key_sequences = true,
    keys = true,
    mapping_attribution = true,
    mapping_uses = true,
    mappings = true,
    mode_transitions = true,
    modes = true,
    ranking_limit = true,
    repeated_key_presses = true,
    repeated_key_runs = true,
    repeated_keys = true,
    schema_version = true,
    sequence_keys = true,
    sessions = true,
    text_keys = true,
    text_runs = true,
    total_session_duration_ms = true,
    unique_keys = true,
    unique_mappings = true,
    unique_repeated_keys = true,
  }
  for key in pairs(decoded.summary) do
    if type(key) ~= "string" or not summary_fields[key] then
      return false, "preview output contains a forbidden field"
    end
  end
  local instructions = decoded.instructions
  if type(instructions) ~= "table"
    or instructions.evidence_required ~= true
    or instructions.collision_check_required ~= true
    or instructions.privacy_boundary ~= PREVIEW_PRIVACY_BOUNDARY
    or type(instructions.action_kinds) ~= "table"
  then
    return false, "preview output has invalid analysis instructions"
  end
  if #instructions.action_kinds ~= #PREVIEW_ACTION_KINDS then
    return false, "preview output has invalid analysis instructions"
  end
  for index, action_kind in ipairs(PREVIEW_ACTION_KINDS) do
    if instructions.action_kinds[index] ~= action_kind then
      return false, "preview output has invalid analysis instructions"
    end
  end
  for key in pairs(instructions) do
    if key ~= "action_kinds"
      and key ~= "collision_check_required"
      and key ~= "evidence_required"
      and key ~= "privacy_boundary"
    then
      return false, "preview output has an unexpected instruction field"
    end
  end

  local function reject_forbidden(value)
    if type(value) == "string" then
      local lower = string.lower(value)
      local safe_bracket_token = value == "<C-/>"
        or value == "<A-/>"
        or value == "<M-/>"
        or value == "<S-/>"
        or value == "<C-\\>"
        or value == "<A-\\>"
        or value == "<M-\\>"
        or value == "<S-\\>"
      if (string.sub(value, 1, 1) == "/" and value ~= "/")
        or string.match(value, "^%a:[/\\]")
        or (value ~= "/"
          and string.find(value, "/", 1, true) ~= nil
          and not safe_bracket_token)
        or (string.find(value, "\\", 1, true) ~= nil and not safe_bracket_token)
        or string.find(lower, "/users/", 1, true) ~= nil
        or string.find(lower, ".env", 1, true) ~= nil
        or string.find(lower, "secret", 1, true) ~= nil
        or string.find(lower, "credential", 1, true) ~= nil
      then
        return false
      end
      return true
    end
    if type(value) ~= "table" then
      return true
    end
    for key, nested in pairs(value) do
      if type(key) == "number" then
        if key < 1 or key ~= math.floor(key) then
          return false
        end
      elseif type(key) ~= "string" or not KNOWN_PREVIEW_KEYS[key] then
        return false
      end
      if type(key) == "string" and FORBIDDEN_PREVIEW_KEYS[key] then
        return false
      end
      if not reject_forbidden(nested) then
        return false
      end
    end
    return true
  end
  if not reject_forbidden(decoded) then
    return false, "preview output contains a forbidden field"
  end
  return true, nil, decoded
end

local function valid_mapping_id(value)
  if type(value) ~= "string" or #value ~= 75 or string.sub(value, 1, 11) ~= "mapping-v1:" then
    return false
  end
  return string.match(string.sub(value, 12), "^[0-9a-f]+$") ~= nil
end

is_array = function(value, maximum)
  if type(value) ~= "table" or (maximum ~= nil and #value > maximum) then
    return false
  end
  local count = 0
  for key in pairs(value) do
    if type(key) ~= "number" or key < 1 or key ~= math.floor(key) then
      return false
    end
    count = count + 1
  end
  return count == #value
end

local function is_counter(value)
  return type(value) == "number"
    and value >= 0
    and value <= MAX_EXACT_JSON_INTEGER
    and value == math.floor(value)
end

local function has_only_fields(value, fields)
  if type(value) ~= "table" then
    return false
  end
  for key in pairs(value) do
    if type(key) ~= "string" or not fields[key] then
      return false
    end
  end
  return true
end

local function validate_histogram(value, labels)
  if not is_array(value, #labels) or #value ~= #labels then
    return false
  end
  for index, bucket in ipairs(value) do
    if not has_only_fields(bucket, { bucket = true, count = true })
      or bucket.bucket ~= labels[index]
      or not is_counter(bucket.count)
    then
      return false
    end
  end
  return true
end

local function valid_summary_mode(value)
  return value == "normal"
    or value == "visual"
    or value == "operator_pending"
    or value == "insert"
    or value == "command"
    or value == "search"
    or value == "other"
end

validate_analysis_summary = function(summary)
  if type(summary) ~= "table"
    or summary.schema_version ~= contract_versions.analysis_summary
    or summary.ranking_limit ~= 100
  then
    return false
  end
  for _, field in ipairs({
    "sessions",
    "events",
    "total_session_duration_ms",
    "key_sequences",
    "sequence_keys",
    "text_runs",
    "text_keys",
    "mode_transitions",
    "mapping_uses",
    "repeated_key_runs",
    "repeated_key_presses",
    "unique_keys",
    "unique_mappings",
    "unique_repeated_keys",
  }) do
    if not is_counter(summary[field]) then
      return false
    end
  end
  if not is_array(summary.modes, 100)
    or not is_array(summary.keys, 100)
    or not is_array(summary.mappings, 100)
    or not is_array(summary.repeated_keys, 100)
    or type(summary.ergonomics) ~= "table"
  then
    return false
  end
  for _, mode in ipairs(summary.modes) do
    if not has_only_fields(mode, { mode = true, sequences = true, keys = true })
      or not valid_summary_mode(mode.mode)
      or not is_counter(mode.sequences)
      or not is_counter(mode.keys)
    then
      return false
    end
  end
  for _, key in ipairs(summary.keys) do
    if not has_only_fields(key, { key = true, count = true })
      or not safe_preview_token(key.key)
      or not is_counter(key.count)
    then
      return false
    end
  end
  for _, mapping in ipairs(summary.mappings) do
    if not has_only_fields(mapping, { mapping_id = true, count = true })
      or not valid_mapping_id(mapping.mapping_id)
      or not is_counter(mapping.count)
    then
      return false
    end
  end
  for _, key in ipairs(summary.repeated_keys) do
    if not has_only_fields(key, { key = true, runs = true, presses = true })
      or not safe_preview_token(key.key)
      or not is_counter(key.runs)
      or not is_counter(key.presses)
    then
      return false
    end
  end
  local ergonomics = summary.ergonomics
  if not has_only_fields(ergonomics, {
      contract_version = true,
      candidate_limit = true,
      thresholds = true,
      distributions = true,
      operations = true,
      count_prefixes = true,
      mode_transitions = true,
      repeated_motions = true,
      mapping_coverage = true,
      candidates = true,
    })
    or ergonomics.contract_version ~= contract_versions.ergonomics or ergonomics.candidate_limit ~= 100
    or not has_only_fields(ergonomics.thresholds, {
      minimum_candidate_sessions = true,
      minimum_candidate_sequence_keys = true,
      minimum_candidate_observations = true,
    })
    or ergonomics.thresholds.minimum_candidate_sessions ~= 3
    or ergonomics.thresholds.minimum_candidate_sequence_keys ~= 100
    or ergonomics.thresholds.minimum_candidate_observations ~= 3
    or not is_array(ergonomics.candidates, 100)
  then
    return false
  end
  local distributions = ergonomics.distributions
  if not has_only_fields(distributions, {
      histogram_version = true,
      session_duration_ms = true,
      sequence_length_keys = true,
      average_inter_key_latency_ms = true,
    })
    or distributions.histogram_version ~= contract_versions.histogram
    or not validate_histogram(distributions.session_duration_ms, { "0-1s", "1-10s", "10-60s", "1-5m", "over-5m" })
    or not validate_histogram(distributions.sequence_length_keys, { "1", "2", "3-4", "5-8", "9-16", "17-32", "33-plus" })
    or not validate_histogram(distributions.average_inter_key_latency_ms, { "0-50ms", "50-100ms", "100-250ms", "250-500ms", "over-500ms" })
  then
    return false
  end
  local operations = ergonomics.operations
  local count_prefixes = ergonomics.count_prefixes
  if not has_only_fields(operations, {
      token_set_version = true,
      undo = true,
      redo = true,
      ["repeat"] = true,
      search_start = true,
      search_navigation = true,
    }) or operations.token_set_version ~= contract_versions.operation_token_set
    or not has_only_fields(count_prefixes, {
      token_set_version = true,
      occurrences = true,
      digit_presses = true,
    }) or count_prefixes.token_set_version ~= contract_versions.count_prefix_token_set
  then
    return false
  end
  for _, field in ipairs({ "undo", "redo", "repeat", "search_start", "search_navigation" }) do
    if not is_counter(operations[field]) then
      return false
    end
  end
  if not is_counter(count_prefixes.occurrences) or not is_counter(count_prefixes.digit_presses)
    or not is_array(ergonomics.mode_transitions, 100)
    or type(ergonomics.repeated_motions) ~= "table"
    or ergonomics.repeated_motions.token_set_version ~= contract_versions.directional_motion_token_set
    or not is_array(ergonomics.repeated_motions.items, 100)
    or type(ergonomics.mapping_coverage) ~= "table"
  then
    return false
  end
  for _, transition in ipairs(ergonomics.mode_transitions) do
    if not has_only_fields(transition, { from = true, to = true, count = true })
      or not valid_summary_mode(transition.from)
      or not valid_summary_mode(transition.to)
      or not is_counter(transition.count)
    then
      return false
    end
  end
  local motions = { h = true, j = true, k = true, l = true, w = true, b = true, e = true }
  for _, motion in ipairs(ergonomics.repeated_motions.items) do
    if not has_only_fields(motion, { motion = true, runs = true, presses = true })
      or not motions[motion.motion]
      or not is_counter(motion.runs)
      or not is_counter(motion.presses)
    then
      return false
    end
  end
  for _, candidate in ipairs(ergonomics.candidates) do
    local guard = type(candidate) == "table" and candidate.guard or nil
    local measurements = type(candidate) == "table" and candidate.measurements or nil
    if not has_only_fields(candidate, {
      candidate_id = true,
      kind = true,
      kind_version = true,
      observations = true,
      measurements = true,
      guard = true,
    })
      or not has_only_fields(guard, {
        observed_sessions = true,
        observed_sequence_keys = true,
        required_sessions = true,
        required_sequence_keys = true,
        required_observations = true,
      })
      or candidate.kind_version ~= contract_versions.candidate_kind
      or not is_counter(candidate.observations)
      or not is_counter(guard.observed_sessions)
      or not is_counter(guard.observed_sequence_keys)
      or guard.required_sessions ~= 3
      or guard.required_sequence_keys ~= 100
      or guard.required_observations ~= 3
      or type(measurements) ~= "table"
    then
      return false
    end
    local allowed_measurements
    if candidate.kind == "repeated_motion" and string.match(candidate.candidate_id or "", "^repeated%-motion%-%a$") then
      local motion = string.sub(candidate.candidate_id, -1)
      if not motions[motion] then
        return false
      end
      allowed_measurements = { presses = true, runs = true }
    elseif candidate.kind == "current_mapping_unobserved_in_sample" then
      local mapping_id = string.match(candidate.candidate_id or "", "^mapping%-unobserved%-v1:(mapping%-v1:.+)$")
      if not valid_mapping_id(mapping_id) then
        return false
      end
      allowed_measurements = { observed_uses = true, sampled_sessions = true }
    else
      return false
    end
    for name, value in pairs(measurements) do
      if type(name) ~= "string" or not allowed_measurements[name] or not is_counter(value) then
        return false
      end
    end
  end
  for _, field in ipairs({ "total_snapshot_mappings", "observed_mappings", "unobserved_mappings" }) do
    if not is_counter(ergonomics.mapping_coverage[field]) then
      return false
    end
  end
  if not has_only_fields(ergonomics.repeated_motions, { token_set_version = true, items = true })
    or not has_only_fields(ergonomics.mapping_coverage, {
      snapshot_version = true,
      total_snapshot_mappings = true,
      observed_mappings = true,
      unobserved_mappings = true,
    })
    or (ergonomics.mapping_coverage.snapshot_version ~= nil
      and ergonomics.mapping_coverage.snapshot_version ~= contract_versions.keymap_snapshot)
  then
    return false
  end
  return true
end

local LEFT_SEARCH_KEY_BOUNDARIES = {
  ["`"] = true,
  ["'"] = true,
  ['"'] = true,
  ["("] = true,
  ["["] = true,
  ["{"] = true,
  ["<"] = true,
}
local RIGHT_SEARCH_KEY_BOUNDARIES = {
  ["`"] = true,
  ["'"] = true,
  ['"'] = true,
  [")"] = true,
  ["]"] = true,
  ["}"] = true,
  [">"] = true,
  [","] = true,
  ["."] = true,
  [";"] = true,
  [":"] = true,
  ["!"] = true,
  ["?"] = true,
  ["…"] = true,
}
local SAFE_MODIFIER_SLASH_TOKENS = {
  ["<C-/>"] = true,
  ["<A-/>"] = true,
  ["<M-/>"] = true,
  ["<S-/>"] = true,
  ["<C-\\>"] = true,
  ["<A-\\>"] = true,
  ["<M-\\>"] = true,
  ["<S-\\>"] = true,
}

local function first_utf8_character(value)
  if value == nil or value == "" then
    return nil
  end
  local byte_index = vim.str_byteindex(value, 1)
  return string.sub(value, 1, byte_index)
end

local function last_utf8_character(value)
  if value == nil or value == "" then
    return nil
  end
  local byte_index = vim.str_byteindex(value, vim.str_utfindex(value) - 1)
  return string.sub(value, byte_index + 1)
end

local function is_slash_separated_key_alternative(token)
  if SAFE_MODIFIER_SLASH_TOKENS[token] ~= nil
    or string.sub(token, 1, 1) == "/"
    or string.sub(token, -1) == "/"
    or string.find(token, "//", 1, true) ~= nil
  then
    return SAFE_MODIFIER_SLASH_TOKENS[token] == true
  end
  local segments = 0
  for segment in string.gmatch(token, "[^/]+") do
    segments = segments + 1
    if vim.str_utfindex(segment) ~= 1 then
      return false
    end
  end
  return segments >= 2 and segments <= 8
end

local function contains_non_standalone_slash(value)
  local offset = 1
  while true do
    local slash = string.find(value, "/", offset, true)
    if slash == nil then
      return false
    end
    local previous_text = slash > 1 and string.sub(value, 1, slash - 1) or nil
    local following_text = slash < #value and string.sub(value, slash + 1) or nil
    local previous = last_utf8_character(previous_text)
    local following = first_utf8_character(following_text)
    local left_safe = previous == nil or string.match(previous, "%s") ~= nil or LEFT_SEARCH_KEY_BOUNDARIES[previous]
    local right_safe = following == nil or string.match(following, "%s") ~= nil or RIGHT_SEARCH_KEY_BOUNDARIES[following]
    if not left_safe or not right_safe then
      local token_start = slash
      while token_start > 1 and string.match(string.sub(value, token_start - 1, token_start - 1), "%s") == nil do
        token_start = token_start - 1
      end
      local token_end = slash
      while token_end < #value and string.match(string.sub(value, token_end + 1, token_end + 1), "%s") == nil do
        token_end = token_end + 1
      end
      local token = string.sub(value, token_start, token_end)
      if not is_slash_separated_key_alternative(token) then
        return true
      end
    end
    offset = slash + 1
  end
end

local function safe_output_text(value, maximum)
  if type(value) ~= "string" or #value < 1 or vim.str_utfindex(value) > maximum then
    return false
  end
  if string.find(value, "[%z\1-\31\127]", 1) ~= nil then
    return false
  end
  local lower = string.lower(value)
  return not contains_non_standalone_slash(value)
    and string.find(value, "\\", 1, true) == nil
    and string.find(lower, ".env", 1, true) == nil
    and string.find(lower, "secret", 1, true) == nil
    and string.find(lower, "credential", 1, true) == nil
    and string.find(lower, "password", 1, true) == nil
    and string.find(lower, "api_key", 1, true) == nil
    and string.find(lower, "raw_log", 1, true) == nil
    and string.find(lower, "session_id", 1, true) == nil
    and string.find(lower, "project_id", 1, true) == nil
    and string.find(lower, "file://", 1, true) == nil
end

safe_preview_token = function(value)
  if type(value) ~= "string" or value == "" or #value > 256 then
    return false
  end
  local first_closing = string.find(value, ">", 1, true)
  local canonical = vim.str_utfindex(value) == 1
    or (string.sub(value, 1, 1) == "<" and first_closing == #value)
  if not canonical then
    return false
  end
  return value == "<C-/>"
    or value == "<A-/>"
    or value == "<M-/>"
    or value == "<S-/>"
    or value == "<C-\\>"
    or value == "<A-\\>"
    or value == "<M-\\>"
    or value == "<S-\\>"
    or safe_output_text(value, 256)
end

expected_snapshot_mapping_id = function(mapping)
  local lhs_count = tostring(#mapping.lhs)
  local preimage = table.concat({
    #"mapping-v1" .. ":mapping-v1",
    #mapping.mode .. ":" .. mapping.mode,
    #mapping.scope .. ":" .. mapping.scope,
    #lhs_count .. ":" .. lhs_count,
  })
  for _, token in ipairs(mapping.lhs) do
    preimage = preimage .. #token .. ":" .. token
  end
  return "mapping-v1:" .. vim.fn.sha256(preimage)
end

local function mapping_precedes(left, right)
  if left.mode ~= right.mode then
    return left.mode < right.mode
  end
  for index = 1, math.min(#left.lhs, #right.lhs) do
    if left.lhs[index] ~= right.lhs[index] then
      return left.lhs[index] < right.lhs[index]
    end
  end
  if #left.lhs ~= #right.lhs then
    return #left.lhs < #right.lhs
  end
  if left.scope ~= right.scope then
    return left.scope < right.scope
  end
  return left.mapping_id < right.mapping_id
end

validate_snapshot_context = function(summary, snapshot)
  if snapshot == nil then
    return summary.mapping_attribution == nil
      and summary.ergonomics.mapping_coverage.snapshot_version == nil
  end
  if type(snapshot) ~= "table"
    or not has_only_fields(snapshot, { snapshot_version = true, mappings = true })
    or snapshot.snapshot_version ~= contract_versions.keymap_snapshot
    or not is_array(snapshot.mappings, 4096)
    or type(summary.mapping_attribution) ~= "table"
  then
    return false
  end
  local snapshot_by_id = {}
  local previous_mapping = nil
  for _, mapping in ipairs(snapshot.mappings) do
    if not has_only_fields(mapping, { mapping_id = true, mode = true, scope = true, lhs = true })
      or not valid_mapping_id(mapping.mapping_id)
      or (mapping.mode ~= "normal" and mapping.mode ~= "visual" and mapping.mode ~= "operator_pending")
      or (mapping.scope ~= "global" and mapping.scope ~= "buffer")
      or not is_array(mapping.lhs, 64)
      or (previous_mapping ~= nil and not mapping_precedes(previous_mapping, mapping))
      or expected_snapshot_mapping_id(mapping) ~= mapping.mapping_id
    then
      return false
    end
    for _, token in ipairs(mapping.lhs) do
      if not safe_preview_token(token) then
        return false
      end
    end
    snapshot_by_id[mapping.mapping_id] = mapping
    previous_mapping = mapping
  end
  local coverage = summary.ergonomics.mapping_coverage
  if coverage.snapshot_version ~= snapshot.snapshot_version
    or coverage.total_snapshot_mappings ~= #snapshot.mappings
    or coverage.observed_mappings + coverage.unobserved_mappings ~= coverage.total_snapshot_mappings
  then
    return false
  end
  local attribution = summary.mapping_attribution
  if not has_only_fields(attribution, { snapshot_version = true, mappings = true, collisions = true })
    or attribution.snapshot_version ~= contract_versions.keymap_snapshot
    or not is_array(attribution.mappings, 4096)
    or #attribution.mappings < #snapshot.mappings
    or not is_array(attribution.collisions, 4096)
  then
    return false
  end
  local attribution_ids = {}
  for _, mapping in ipairs(attribution.mappings) do
    local collision_mapping_ids = type(mapping) == "table" and (mapping.collision_mapping_ids or {}) or nil
    if not has_only_fields(mapping, {
        mapping_id = true,
        status = true,
        count = true,
        mode = true,
        scope = true,
        lhs = true,
        collision_mapping_ids = true,
      })
      or not valid_mapping_id(mapping.mapping_id)
      or attribution_ids[mapping.mapping_id]
      or not is_counter(mapping.count)
      or not is_array(collision_mapping_ids, 4096)
      or (mapping.status ~= "observed"
        and mapping.status ~= "observed_not_in_snapshot"
        and mapping.status ~= "unobserved_in_sample")
    then
      return false
    end
    attribution_ids[mapping.mapping_id] = true
    local snapshot_mapping = snapshot_by_id[mapping.mapping_id]
    if mapping.status == "observed_not_in_snapshot" then
      if snapshot_mapping ~= nil or mapping.count < 1 or mapping.mode ~= nil or mapping.scope ~= nil or mapping.lhs ~= nil then
        return false
      end
    elseif snapshot_mapping == nil
      or (mapping.status == "observed" and mapping.count < 1)
      or (mapping.status == "unobserved_in_sample" and mapping.count ~= 0)
      or mapping.mode ~= snapshot_mapping.mode
      or mapping.scope ~= snapshot_mapping.scope
      or not is_array(mapping.lhs, 64)
      or not vim.deep_equal(mapping.lhs, snapshot_mapping.lhs)
    then
      return false
    end
    for _, collision_mapping_id in ipairs(collision_mapping_ids) do
      if not valid_mapping_id(collision_mapping_id) or snapshot_by_id[collision_mapping_id] == nil then
        return false
      end
    end
  end
  for _, collision in ipairs(attribution.collisions) do
    local global_mapping = type(collision) == "table" and snapshot_by_id[collision.global_mapping_id] or nil
    local buffer_mapping = type(collision) == "table" and snapshot_by_id[collision.buffer_mapping_id] or nil
    if not has_only_fields(collision, {
        kind = true,
        mode = true,
        lhs = true,
        global_mapping_id = true,
        buffer_mapping_id = true,
      })
      or collision.kind ~= "potential_buffer_shadowing"
      or type(collision.mode) ~= "string"
      or not is_array(collision.lhs, 64)
      or global_mapping == nil
      or buffer_mapping == nil
      or collision.mode ~= global_mapping.mode
      or global_mapping.scope ~= "global"
      or buffer_mapping.scope ~= "buffer"
      or buffer_mapping.mode ~= global_mapping.mode
      or not vim.deep_equal(global_mapping.lhs, collision.lhs)
      or not vim.deep_equal(buffer_mapping.lhs, collision.lhs)
    then
      return false
    end
    for _, token in ipairs(collision.lhs) do
      if not safe_preview_token(token) then
        return false
      end
    end
  end
  return true
end

local function expected_metric(summary, metric)
  if summary[metric] ~= nil then
    return summary[metric]
  end
  local coverage = summary.ergonomics.mapping_coverage
  if coverage[metric] ~= nil then
    return coverage[metric]
  end
  local count_prefixes = summary.ergonomics.count_prefixes
  if metric == "count_prefix_occurrences" then
    return count_prefixes.occurrences
  end
  if metric == "count_prefix_digit_presses" then
    return count_prefixes.digit_presses
  end
  return nil
end

local function lhs_collides(existing, proposed)
  local shared = math.min(#existing, #proposed)
  for index = 1, shared do
    if existing[index] ~= proposed[index] then
      return false
    end
  end
  return true
end

local function validate_codex_suggestions(contents, preview_payload)
  if type(contents) ~= "string" or #contents > MAX_CODEX_BYTES then
    return false, "Codex output exceeds the size limit"
  end
  local document = strict_json.decode(contents)
  if type(document) ~= "table" or document.schema_version ~= contract_versions.codex_suggestions then
    return false, "Codex returned invalid structured suggestions"
  end
  if type(preview_payload) ~= "table" or not validate_analysis_summary(preview_payload.summary) then
    return false, "Codex evidence has no valid sanitized source"
  end
  local top_level = { schema_version = true, suggestions = true }
  for key in pairs(document) do
    if type(key) ~= "string" or not top_level[key] then
      return false, "Codex returned an unexpected field"
    end
  end
  if not is_array(document.suggestions, MAX_CODEX_SUGGESTIONS) then
    return false, "Codex returned too many suggestions"
  end
  local actions = {
    learn_existing = true,
    add_mapping = true,
    change_mapping = true,
    no_change = true,
  }
  local metrics = {
    sessions = true,
    events = true,
    total_session_duration_ms = true,
    key_sequences = true,
    sequence_keys = true,
    text_runs = true,
    text_keys = true,
    mode_transitions = true,
    mapping_uses = true,
    repeated_key_runs = true,
    repeated_key_presses = true,
    unique_keys = true,
    unique_mappings = true,
    unique_repeated_keys = true,
    observed_mappings = true,
    unobserved_mappings = true,
    total_snapshot_mappings = true,
    count_prefix_occurrences = true,
    count_prefix_digit_presses = true,
  }
  for _, suggestion in ipairs(document.suggestions) do
    if type(suggestion) ~= "table"
      or not actions[suggestion.action]
      or not safe_output_text(suggestion.title, 256)
      or not safe_output_text(suggestion.rationale, 4096)
    then
      return false, "Codex returned an invalid suggestion"
    end
    local allowed = {
      action = true,
      title = true,
      rationale = true,
      mapping = true,
      evidence = true,
      collision_check = true,
    }
    for key in pairs(suggestion) do
      if type(key) ~= "string" or not allowed[key] then
        return false, "Codex returned an unexpected suggestion field"
      end
    end
    local proposal = suggestion.mapping
    if proposal == vim.NIL then
      proposal = nil
    end
    local mapping_action = suggestion.action == "add_mapping" or suggestion.action == "change_mapping"
    if mapping_action then
      local target_mapping_id = type(proposal) == "table" and proposal.target_mapping_id or nil
      if target_mapping_id == vim.NIL then
        target_mapping_id = nil
      end
      if type(proposal) ~= "table"
        or (proposal.mode ~= "normal" and proposal.mode ~= "visual" and proposal.mode ~= "operator_pending")
        or (proposal.scope ~= "global" and proposal.scope ~= "buffer")
        or not is_array(proposal.lhs, 64)
        or #proposal.lhs < 1
        or (suggestion.action == "add_mapping" and target_mapping_id ~= nil)
        or (suggestion.action == "change_mapping" and not valid_mapping_id(target_mapping_id))
      then
        return false, "Codex returned an invalid mapping proposal"
      end
      for key in pairs(proposal) do
        if key ~= "mode" and key ~= "scope" and key ~= "lhs" and key ~= "target_mapping_id" then
          return false, "Codex returned an unexpected mapping proposal field"
        end
      end
      for _, token in ipairs(proposal.lhs) do
        if not safe_preview_token(token) then
          return false, "Codex returned an invalid mapping proposal"
        end
      end
    elseif proposal ~= nil then
      return false, "Codex returned an unexpected mapping proposal"
    end
    if not is_array(suggestion.evidence, MAX_SUGGESTION_EVIDENCE)
      or #suggestion.evidence < 1
    then
      return false, "Codex returned invalid evidence"
    end
    for _, evidence in ipairs(suggestion.evidence) do
      if type(evidence) ~= "table"
        or not metrics[evidence.metric]
        or type(evidence.value) ~= "number"
        or evidence.value < 0
        or evidence.value ~= math.floor(evidence.value)
      then
        return false, "Codex returned invalid evidence"
      end
      if expected_metric(preview_payload.summary, evidence.metric) ~= evidence.value then
        return false, "Codex evidence does not match the sanitized summary"
      end
      for key in pairs(evidence) do
        if key ~= "metric" and key ~= "value" then
          return false, "Codex returned an unexpected evidence field"
        end
      end
    end
    local collision = suggestion.collision_check
    if type(collision) ~= "table" or collision.checked ~= true
      or not is_array(collision.conflicting_mapping_ids, MAX_SUGGESTION_CONFLICTS)
    then
      return false, "Codex returned invalid collision evidence"
    end
    for key in pairs(collision) do
      if key ~= "checked" and key ~= "conflicting_mapping_ids" then
        return false, "Codex returned an unexpected collision field"
      end
    end
    for _, mapping_id in ipairs(collision.conflicting_mapping_ids) do
      if not valid_mapping_id(mapping_id) then
        return false, "Codex returned an invalid mapping ID"
      end
    end
    local snapshot = preview_payload.keymap_snapshot
    local snapshot_ids = {}
    local snapshot_by_id = {}
    if type(snapshot) == "table" then
      if snapshot.snapshot_version ~= contract_versions.keymap_snapshot or not is_array(snapshot.mappings, 4096) then
        return false, "Codex collision evidence has an invalid snapshot"
      end
      for _, mapping in ipairs(snapshot.mappings) do
        if type(mapping) ~= "table"
          or not valid_mapping_id(mapping.mapping_id)
          or (mapping.mode ~= "normal" and mapping.mode ~= "visual" and mapping.mode ~= "operator_pending")
          or (mapping.scope ~= "global" and mapping.scope ~= "buffer")
          or not is_array(mapping.lhs, 64)
        then
          return false, "Codex collision evidence has an invalid snapshot"
        end
        for _, token in ipairs(mapping.lhs) do
          if not safe_preview_token(token) then
            return false, "Codex collision evidence has an invalid snapshot"
          end
        end
        if expected_snapshot_mapping_id(mapping) ~= mapping.mapping_id then
          return false, "Codex collision evidence has an invalid snapshot"
        end
        snapshot_ids[mapping.mapping_id] = true
        snapshot_by_id[mapping.mapping_id] = mapping
      end
    end
    local attribution = preview_payload.summary.mapping_attribution
    if snapshot == nil and attribution ~= nil then
      return false, "Codex collision evidence has an unexpected attribution"
    end
    if snapshot ~= nil and (type(attribution) ~= "table" or not is_array(attribution.collisions, 4096)) then
      return false, "Codex collision evidence has no valid attribution"
    end
    if type(attribution) == "table" then
      if attribution.snapshot_version ~= contract_versions.keymap_snapshot
        or not is_array(attribution.mappings, 4096)
        or #attribution.mappings < (snapshot and #snapshot.mappings or 0)
      then
        return false, "Codex collision evidence has an invalid attribution"
      end
      local attribution_ids = {}
      for _, mapping in ipairs(attribution.mappings) do
        local mapping_collisions = type(mapping) == "table" and (mapping.collision_mapping_ids or {}) or nil
        if type(mapping) ~= "table"
          or not valid_mapping_id(mapping.mapping_id)
          or attribution_ids[mapping.mapping_id]
          or not is_counter(mapping.count)
          or not is_array(mapping_collisions, 4096)
          or (mapping.status ~= "observed"
            and mapping.status ~= "observed_not_in_snapshot"
            and mapping.status ~= "unobserved_in_sample")
        then
          return false, "Codex collision evidence has an invalid attribution"
        end
        attribution_ids[mapping.mapping_id] = true
        local snapshot_mapping = snapshot_by_id[mapping.mapping_id]
        if mapping.status ~= "observed_not_in_snapshot" and snapshot_mapping == nil then
          return false, "Codex collision evidence has an invalid attribution"
        end
        if mapping.status == "observed_not_in_snapshot" then
          if snapshot_mapping ~= nil
            or mapping.count < 1
            or mapping.mode ~= nil
            or mapping.scope ~= nil
            or mapping.lhs ~= nil
          then
            return false, "Codex collision evidence has an invalid attribution"
          end
        else
          if mapping.status == "observed" and mapping.count < 1 then
            return false, "Codex collision evidence has an invalid attribution"
          end
          if mapping.status == "unobserved_in_sample" and mapping.count ~= 0 then
            return false, "Codex collision evidence has an invalid attribution"
          end
          if type(mapping.mode) ~= "string"
            or (mapping.scope ~= "global" and mapping.scope ~= "buffer")
            or not is_array(mapping.lhs, 64)
            or snapshot_mapping.mode ~= mapping.mode
            or snapshot_mapping.scope ~= mapping.scope
            or not vim.deep_equal(snapshot_mapping.lhs, mapping.lhs)
          then
            return false, "Codex collision evidence has an invalid attribution"
          end
        end
        for _, collision_mapping_id in ipairs(mapping_collisions) do
          if not valid_mapping_id(collision_mapping_id) or not snapshot_ids[collision_mapping_id] then
            return false, "Codex collision evidence has an invalid attribution"
          end
        end
      end
    end
    if type(attribution) == "table" and is_array(attribution.collisions, 4096) then
      for _, collision in ipairs(attribution.collisions) do
        if type(collision) ~= "table"
          or collision.kind ~= "potential_buffer_shadowing"
          or type(collision.mode) ~= "string"
          or not is_array(collision.lhs, 64)
          or not valid_mapping_id(collision.global_mapping_id)
          or not valid_mapping_id(collision.buffer_mapping_id)
          or not snapshot_ids[collision.global_mapping_id]
          or not snapshot_ids[collision.buffer_mapping_id]
        then
          return false, "Codex collision evidence has an invalid attribution"
        end
        for _, token in ipairs(collision.lhs) do
          if not safe_preview_token(token) then
            return false, "Codex collision evidence has an invalid attribution"
          end
        end
        local global_mapping = snapshot_by_id[collision.global_mapping_id]
        local buffer_mapping = snapshot_by_id[collision.buffer_mapping_id]
        if collision.mode ~= global_mapping.mode
          or global_mapping.scope ~= "global"
          or buffer_mapping.scope ~= "buffer"
          or buffer_mapping.mode ~= global_mapping.mode
          or not vim.deep_equal(global_mapping.lhs, collision.lhs)
          or not vim.deep_equal(buffer_mapping.lhs, collision.lhs)
        then
          return false, "Codex collision evidence has an invalid attribution"
        end
      end
    end
    local reported = {}
    for _, mapping_id in ipairs(collision.conflicting_mapping_ids) do
      if reported[mapping_id] then
        return false, "Codex returned a duplicate mapping collision"
      end
      reported[mapping_id] = true
    end
    if mapping_action then
      if snapshot == nil then
        return false, "mapping suggestions require a sanitized snapshot"
      end
      local target_mapping_id = proposal.target_mapping_id
      if target_mapping_id == vim.NIL then
        target_mapping_id = nil
      end
      if target_mapping_id ~= nil and snapshot_by_id[target_mapping_id] == nil then
        return false, "Codex returned an unknown mapping change target"
      end
      local expected = {}
      for mapping_id, mapping in pairs(snapshot_by_id) do
        if mapping.mode == proposal.mode
          and lhs_collides(mapping.lhs, proposal.lhs)
          and mapping_id ~= target_mapping_id
        then
          expected[mapping_id] = true
        end
      end
      for mapping_id in pairs(expected) do
        if not reported[mapping_id] then
          return false, "Codex omitted a proposed mapping collision"
        end
      end
      for mapping_id in pairs(reported) do
        if not expected[mapping_id] then
          return false, "Codex reported an unknown mapping collision"
        end
      end
    elseif next(reported) ~= nil then
      return false, "Codex reported a collision without a mapping proposal"
    end
  end
  return true
end

local function protect_directory(fs, path, mode)
  local before, stat_error = fs.fs_lstat(path)
  if before == nil or before.type ~= "directory" then
    return false, stat_error or "path is not a directory"
  end
  local descriptor, open_error = filesystem.open_read(fs, path)
  if descriptor == nil then
    return false, open_error
  end
  local opened, inspect_error = fs.fs_fstat(descriptor)
  local unchanged = opened ~= nil
    and opened.type == "directory"
    and before.dev ~= nil
    and before.ino ~= nil
    and opened.dev == before.dev
    and opened.ino == before.ino
  local protected, protect_error = false, inspect_error
  if unchanged then
    protected, protect_error = fs.fs_fchmod(descriptor, mode)
  end
  local closed, close_error = fs.fs_close(descriptor)
  if not unchanged then
    return false, inspect_error or "directory changed while opening"
  end
  if not protected or not closed then
    return false, protect_error or close_error
  end
  return true
end

local function prepare_codex_directory(fs, mkdir, path)
  local mkdir_ok, mkdir_result = pcall(mkdir, path, "p", OWNER_DIRECTORY)
  if not mkdir_ok or type(mkdir_result) ~= "number" or mkdir_result < 0 then
    return false
  end
  local protected = protect_directory(fs, path, OWNER_DIRECTORY)
  if not protected then
    return false
  end
  local request = fs.fs_scandir(path)
  return request ~= nil and fs.fs_scandir_next(request) == nil
end

local function file_identity(fs, path)
  return filesystem.stat_identity(fs.fs_lstat(path))
end

local function capture_outputs(fs, summary_path, report_path)
  return {
    summary = file_identity(fs, summary_path),
    report = file_identity(fs, report_path),
  }
end

local function validate_report(fs, path)
  local contents, read_error = filesystem.read_bounded(fs, path, MAX_REPORT_BYTES)
  if contents == nil then
    return false, "report.md is unavailable: " .. tostring(read_error)
  end
  if vim.split(contents, "\n", { plain = true })[1] ~= REPORT_HEADER then
    return false, "report.md has an unexpected format"
  end
  return true
end

local function validate_outputs(fs, summary_path, report_path, previous)
  local contents, read_error = filesystem.read_bounded(fs, summary_path, MAX_SUMMARY_BYTES)
  if contents == nil then
    return false, "summary.json is unavailable: " .. tostring(read_error)
  end
  local decoded_ok, summary = pcall(vim.json.decode, contents)
  if not decoded_ok
    or type(summary) ~= "table"
    or contract_versions.report_summary_versions[summary.schema_version] ~= true
    or type(summary.sessions) ~= "number"
    or type(summary.events) ~= "number"
  then
    return false, "summary.json has an unexpected format"
  end
  if previous ~= nil
    and (file_identity(fs, summary_path) == previous.summary or file_identity(fs, report_path) == previous.report)
  then
    return false, "the analyzer did not publish fresh outputs"
  end
  return validate_report(fs, report_path)
end

local function validate_with(validator, ...)
  local call_ok, valid, validation_error = pcall(validator, ...)
  if not call_ok then
    return false, valid
  end
  return valid, validation_error
end

local function assert_nonempty(value, name)
  assert(type(value) == "string" and value ~= "", name .. " must be a non-empty string")
end

local function codex_process_environment()
  local codex_home = vim.env.CODEX_HOME
  if type(codex_home) ~= "string" or codex_home == "" then
    local home = vim.env.HOME
    assert(type(home) == "string" and home ~= "", "HOME is unavailable")
    codex_home = vim.fs.joinpath(home, ".codex")
  end
  return {
    CODEX_HOME = codex_home,
    PATH = type(vim.env.PATH) == "string" and vim.env.PATH or "",
  }
end

local function valid_codex_process_environment(environment)
  if type(environment) ~= "table"
    or type(environment.CODEX_HOME) ~= "string"
    or environment.CODEX_HOME == ""
    or not filesystem.is_absolute_path(environment.CODEX_HOME)
    or type(environment.PATH) ~= "string"
    or environment.PATH == ""
  then
    return false
  end
  local stat = vim.uv.fs_lstat(environment.CODEX_HOME)
  if stat == nil or stat.type ~= "directory" then
    return false
  end
  local count = 0
  for key, value in pairs(environment) do
    if (key ~= "CODEX_HOME" and key ~= "PATH") or type(value) ~= "string" then
      return false
    end
    count = count + 1
  end
  return count == 2
end

local function resolve_codex_binary(binary)
  if filesystem.is_absolute_path(binary) then
    return vim.fn.executable(binary) == 1 and binary or nil
  end
  local resolved = vim.fn.exepath(binary)
  if type(resolved) ~= "string" or resolved == "" or not filesystem.is_absolute_path(resolved) then
    return nil
  end
  return resolved
end

function M.new(options, dependencies)
  local config = options or {}
  assert_nonempty(config.analyzer, "report analyzer")
  assert_nonempty(config.output_directory, "report output directory")
  assert_nonempty(config.session_directory, "report session directory")
  local deps = dependencies or {}
  local fs = deps.fs or vim.uv
  local payload = nil
  if deps.collect_snapshot_payload == nil then
    payload = snapshot_payload.new({
      collector_options = config.collector_options,
    })
  end
  return setmetatable({
    _analyzer = config.analyzer,
    _capture_outputs = deps.capture_outputs or function(summary_path, report_path)
      return capture_outputs(fs, summary_path, report_path)
    end,
    _job = nil,
    _phase = nil,
    _await_confirmation = false,
    _generation = 0,
    _mkdir = deps.mkdir or vim.fn.mkdir,
    _notify_fn = deps.notify or default_notify,
    _open_file = deps.open_file or default_open_file,
    _open_preview = deps.open_preview or default_open_preview,
    _open_suggestions = deps.open_suggestions or deps.open_preview or default_open_suggestions,
    _confirm = deps.confirm or default_confirm,
    _output_directory = config.output_directory,
    _previous_outputs = nil,
    _protect_directory = deps.protect_directory or function(path, mode)
      return protect_directory(fs, path, mode)
    end,
    _collect_snapshot_payload = deps.collect_snapshot_payload or function()
      return payload:collect()
    end,
    _report_path = vim.fs.joinpath(config.output_directory, "report.md"),
    _run = deps.run or process.run,
    _run_codex = deps.run_codex or process.run,
    _run_suggestions = deps.run_suggestions or process.run,
    _supports_process_groups = deps.supports_process_groups or process.supports_process_groups,
    _codex_binary = (config.codex and config.codex.binary) or "codex",
    _resolved_codex_binary = nil,
    _resolved_codex_environment = nil,
    _resolve_codex_binary = deps.resolve_codex_binary or resolve_codex_binary,
    _codex_output_schema = (config.codex and config.codex.output_schema) or default_codex_schema(),
    _codex_working_directory = (config.codex and config.codex.working_directory)
      or vim.fs.joinpath(vim.fn.stdpath("cache"), "key-insights", "codex-empty"),
    _prepare_codex_directory = deps.prepare_codex_directory or function(path)
      return prepare_codex_directory(fs, deps.mkdir or vim.fn.mkdir, path)
    end,
    _codex_environment = deps.codex_environment or codex_process_environment,
    _session_directory = config.session_directory,
    _summary_path = vim.fs.joinpath(config.output_directory, "summary.json"),
    _validate_outputs = deps.validate_outputs or function(summary_path, report_path, previous)
      return validate_outputs(fs, summary_path, report_path, previous)
    end,
    _validate_report = deps.validate_report or function(path)
      return validate_report(fs, path)
    end,
  }, Report)
end

function M.default_directory()
  return vim.fs.joinpath(vim.fn.stdpath("state"), "key-insights", "reports")
end

function Report:status()
  local running = self._job ~= nil or self._phase ~= nil
  return { running = running, job = self._job ~= nil and true or nil, phase = self._phase }
end

function Report:_notify(message, level)
  self._notify_fn(message, level)
end

function Report:_open()
  local ok, open_error = pcall(self._open_file, self._report_path)
  if not ok then
    self:_notify("failed to open report: " .. tostring(open_error), vim.log.levels.ERROR)
    return false
  end
  return true
end

function Report:_complete(result, generation)
  if generation ~= self._generation then
    return
  end
  self._job = nil
  local previous_outputs = self._previous_outputs
  self._previous_outputs = nil
  if type(result) ~= "table" or type(result.code) ~= "number" then
    self:_notify("analyzer returned an invalid process result", vim.log.levels.ERROR)
    return
  end
  if result.code ~= 0 or (type(result.signal) == "number" and result.signal ~= 0) then
    self:_notify("report failed (the analyzer exited unsuccessfully)", vim.log.levels.ERROR)
    return
  end
  local valid, validation_error = validate_with(
    self._validate_outputs,
    self._summary_path,
    self._report_path,
    previous_outputs
  )
  if not valid then
    self:_notify(tostring(validation_error or "analyzer outputs are invalid"), vim.log.levels.ERROR)
    return
  end
  self:_notify("report updated", vim.log.levels.INFO)
  self:_open()
end

function Report:_complete_preview(result, generation)
  if generation ~= self._generation then
    return
  end
  self._job = nil
  if type(result) ~= "table" or type(result.code) ~= "number" then
    self:_notify("preview returned an invalid process result", vim.log.levels.ERROR)
    return
  end
  if result.code ~= 0 or (type(result.signal) == "number" and result.signal ~= 0) then
    self:_notify("preview failed (the analyzer exited unsuccessfully)", vim.log.levels.ERROR)
    return
  end
  local valid, validation_error, preview_payload = validate_preview(result.stdout)
  if not valid then
    self:_notify(tostring(validation_error or "preview output is invalid"), vim.log.levels.ERROR)
    return
  end
  local open_ok, open_error = pcall(self._open_preview, result.stdout)
  if not open_ok then
    self:_notify("failed to open preview: " .. tostring(open_error), vim.log.levels.ERROR)
    return
  end
  if self._await_confirmation then
    self._await_confirmation = false
    if not self._supports_process_groups() then
      self:_notify("Codex analysis requires Unix process-group isolation", vim.log.levels.ERROR)
      return
    end
    self._phase = "awaiting_confirmation"
    local resolve_ok, resolved_binary = pcall(self._resolve_codex_binary, self._codex_binary)
    if not resolve_ok or resolved_binary == nil then
      self._phase = nil
      self:_notify("failed to resolve the configured Codex executable", vim.log.levels.ERROR)
      return
    end
    local environment_ok, environment = pcall(self._codex_environment)
    if not environment_ok or not valid_codex_process_environment(environment) then
      self._phase = nil
      self:_notify("failed to prepare the isolated Codex environment", vim.log.levels.ERROR)
      return
    end
    self._resolved_codex_binary = resolved_binary
    self._resolved_codex_environment = vim.deepcopy(environment)
    local confirm_ok, confirm_error = pcall(self._confirm, function(confirmed)
      if generation ~= self._generation or self._phase ~= "awaiting_confirmation" then
        return
      end
      self._phase = nil
      if not confirmed then
        self._resolved_codex_binary = nil
        self._resolved_codex_environment = nil
        self:_notify("Codex analysis cancelled", vim.log.levels.INFO)
        return
      end
      self:_start_codex(result.stdout, generation, preview_payload)
    end, resolved_binary)
    if not confirm_ok then
      self._phase = nil
      self._resolved_codex_binary = nil
      self._resolved_codex_environment = nil
      self:_notify("failed to request Codex confirmation: " .. tostring(confirm_error), vim.log.levels.ERROR)
      return
    end
  end
  self:_notify("sanitized Codex preview is ready", vim.log.levels.INFO)
end

function Report:_start_codex(payload, generation, preview_payload)
  if type(payload) ~= "string" or #payload > MAX_CODEX_BYTES then
    self:_notify("Codex payload exceeds the size limit", vim.log.levels.ERROR)
    return false
  end
  if not self._supports_process_groups() then
    self:_notify("Codex analysis requires Unix process-group isolation", vim.log.levels.ERROR)
    return false
  end
  local codex_binary = self._resolved_codex_binary
  local environment = self._resolved_codex_environment
  self._resolved_codex_binary = nil
  self._resolved_codex_environment = nil
  if codex_binary == nil then
    local resolve_ok
    resolve_ok, codex_binary = pcall(self._resolve_codex_binary, self._codex_binary)
    if not resolve_ok then
      codex_binary = nil
    end
  end
  if codex_binary == nil then
    self:_notify("failed to resolve the configured Codex executable", vim.log.levels.ERROR)
    return false
  end
  local prepare_ok, prepared = pcall(self._prepare_codex_directory, self._codex_working_directory)
  if not prepare_ok or not prepared then
    self:_notify("failed to prepare the isolated Codex working directory", vim.log.levels.ERROR)
    return false
  end
  if environment == nil then
    local environment_ok
    environment_ok, environment = pcall(self._codex_environment)
    if not environment_ok or not valid_codex_process_environment(environment) then
      self:_notify("failed to prepare the isolated Codex environment", vim.log.levels.ERROR)
      return false
    end
  end
  local argv = {
    codex_binary,
    "exec",
    "--ephemeral",
    "--ignore-user-config",
    "--ignore-rules",
    "--strict-config",
    "--skip-git-repo-check",
    "--cd",
    self._codex_working_directory,
    "--config",
    'shell_environment_policy.inherit="none"',
    "--config",
    'approval_policy="never"',
    "--config",
    'default_permissions="key-insights-payload-only"',
    "--config",
    'permissions.key-insights-payload-only.filesystem={":root"="deny",":minimal"="read"}',
    "--config",
    "permissions.key-insights-payload-only.network.enabled=false",
    "--output-schema",
    self._codex_output_schema,
  }
  self._phase = "codex"
  self._job = true
  local completed = false
  local run_ok, job = pcall(self._run_codex, argv, function(codex_result)
    completed = true
    if generation ~= self._generation or self._phase ~= "codex" then
      return
    end
    self._job = nil
    if type(codex_result) ~= "table" or type(codex_result.code) ~= "number" then
      self._phase = nil
      self:_notify("Codex returned an invalid process result", vim.log.levels.ERROR)
      return
    end
    if codex_result.code ~= 0 or (type(codex_result.signal) == "number" and codex_result.signal ~= 0) then
      self._phase = nil
      self:_notify("Codex analysis failed (the subprocess exited unsuccessfully)", vim.log.levels.ERROR)
      return
    end
    if type(codex_result.stdout) ~= "string" or #codex_result.stdout > MAX_CODEX_BYTES then
      self._phase = nil
      self:_notify("Codex output exceeds the size limit", vim.log.levels.ERROR)
      return
    end
    local valid, validation_error = validate_codex_suggestions(codex_result.stdout, preview_payload)
    if not valid then
      self._phase = nil
      self:_notify(tostring(validation_error or "Codex returned invalid structured suggestions"), vim.log.levels.ERROR)
      return
    end
    self:_start_suggestion_render(codex_result.stdout, generation)
  end, payload, {
    clear_env = true,
    env = environment,
  })
  if not run_ok or (not job and not completed) then
    self._job = nil
    self._phase = nil
    self:_notify("failed to start Codex: " .. tostring(job), vim.log.levels.ERROR)
    return false
  end
  if not completed then
    self._job = job
  end
  return true
end

function Report:_complete_suggestion_render(result, generation)
  if generation ~= self._generation or self._phase ~= "rendering_suggestions" then
    return
  end
  self._job = nil
  self._phase = nil
  if type(result) ~= "table" or type(result.code) ~= "number" then
    self:_notify("suggestion renderer returned an invalid process result", vim.log.levels.ERROR)
    return
  end
  if result.code ~= 0 or (type(result.signal) == "number" and result.signal ~= 0) then
    self:_notify("suggestion rendering failed", vim.log.levels.ERROR)
    return
  end
  if type(result.stdout) ~= "string"
    or #result.stdout > MAX_RENDERED_SUGGESTIONS_BYTES
    or vim.split(result.stdout, "\n", { plain = true })[1] ~= "# Codex suggestions"
  then
    self:_notify("suggestion renderer returned invalid Markdown", vim.log.levels.ERROR)
    return
  end
  local open_ok, open_error = pcall(self._open_suggestions, result.stdout)
  if not open_ok then
    self:_notify("failed to open Codex suggestions: " .. tostring(open_error), vim.log.levels.ERROR)
    return
  end
  self:_notify("Codex suggestions are ready", vim.log.levels.INFO)
end

function Report:_start_suggestion_render(contents, generation)
  self._phase = "rendering_suggestions"
  self._job = true
  local completed = false
  local run_ok, job = pcall(self._run_suggestions, {
    self._analyzer,
    "suggestions",
    self._summary_path,
    "--input",
    "-",
    "--output",
    "-",
  }, function(result)
    completed = true
    self:_complete_suggestion_render(result, generation)
  end, contents, { max_stdout_bytes = MAX_RENDERED_SUGGESTIONS_BYTES + 1 })
  if not run_ok or (not job and not completed) then
    self._job = nil
    self._phase = nil
    self:_notify("failed to start the suggestion renderer", vim.log.levels.ERROR)
    return false
  end
  if not completed then
    self._job = job
  end
  return true
end

function Report:start()
  if self._job ~= nil or self._phase ~= nil then
    self:_notify("a report is already running", vim.log.levels.WARN)
    return false
  end
  local mkdir_ok, mkdir_result = pcall(self._mkdir, self._output_directory, "p", OWNER_DIRECTORY)
  if not mkdir_ok or type(mkdir_result) ~= "number" or mkdir_result < 0 then
    self:_notify("failed to create the report directory", vim.log.levels.ERROR)
    return false
  end
  local protect_ok, protected = pcall(self._protect_directory, self._output_directory, OWNER_DIRECTORY)
  if not protect_ok or not protected then
    self:_notify("failed to protect the report directory", vim.log.levels.ERROR)
    return false
  end
  local capture_ok, previous_outputs = pcall(self._capture_outputs, self._summary_path, self._report_path)
  if not capture_ok then
    self:_notify("failed to inspect existing report outputs", vim.log.levels.ERROR)
    return false
  end
  self._previous_outputs = previous_outputs
  local collect_ok, snapshot = pcall(self._collect_snapshot_payload)
  if not collect_ok or type(snapshot) ~= "string" or snapshot == "" then
    self._previous_outputs = nil
    self:_notify("failed to collect keymap snapshot", vim.log.levels.ERROR)
    return false
  end
  local argv = {
    self._analyzer,
    "analyze",
    "--session-dir",
    self._session_directory,
    "--summary",
    self._summary_path,
    "--report",
    self._report_path,
    "--keymap-snapshot",
    "-",
  }
  self._generation = self._generation + 1
  local generation = self._generation
  self._job = true
  local completed = false
  local run_ok, job = pcall(self._run, argv, function(result)
    completed = true
    self:_complete(result, generation)
  end, snapshot)
  if not run_ok or (not job and not completed) then
    self._job = nil
    self._previous_outputs = nil
    self:_notify("failed to start the analyzer: " .. tostring(job), vim.log.levels.ERROR)
    return false
  end
  if not completed then
    self._job = job
  end
  return true
end

function Report:preview()
  if self._job ~= nil or self._phase ~= nil then
    self:_notify("a report or preview is already running", vim.log.levels.WARN)
    return false
  end
  local argv = {
    self._analyzer,
    "preview",
    self._summary_path,
  }
  vim.list_extend(argv, { "--output", "-" })
  self._generation = self._generation + 1
  local generation = self._generation
  self._job = true
  local completed = false
  local run_ok, job = pcall(self._run, argv, function(result)
    completed = true
    self:_complete_preview(result, generation)
  end)
  if not run_ok or (not job and not completed) then
    self._job = nil
    self:_notify("failed to start the preview: " .. tostring(job), vim.log.levels.ERROR)
    return false
  end
  if not completed then
    self._job = job
  end
  return true
end

function Report:analyze()
  if self._job ~= nil or self._phase ~= nil then
    self:_notify("a report or analysis is already running", vim.log.levels.WARN)
    return false
  end
  self._await_confirmation = true
  local started = self:preview()
  if not started then
    self._await_confirmation = false
  end
  return started
end

function Report:shutdown()
  if self._job == nil and self._phase == nil then
    return false
  end
  self._generation = self._generation + 1
  local job = self._job
  self._job = nil
  self._phase = nil
  self._await_confirmation = false
  self._resolved_codex_binary = nil
  self._resolved_codex_environment = nil
  if type(job) == "table" and type(job.kill) == "function" then
    pcall(job.kill, job, 9)
  end
  self._previous_outputs = nil
  return true
end

function Report:open()
  local valid, validation_error = validate_with(self._validate_report, self._report_path)
  if not valid then
    self:_notify(tostring(validation_error or "report.md is invalid"), vim.log.levels.ERROR)
    return false
  end
  return self:_open()
end

return M
