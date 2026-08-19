local filesystem = require("key-insights.filesystem")
local process = require("key-insights.process")
local snapshot_payload = require("key-insights.snapshot_payload")

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

local function default_open_preview(contents)
  local buffer = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_buf_set_lines(buffer, 0, -1, false, vim.split(contents, "\n", { plain = true }))
  vim.bo[buffer].buftype = "nofile"
  vim.bo[buffer].bufhidden = "wipe"
  vim.bo[buffer].swapfile = false
  vim.bo[buffer].filetype = "json"
  vim.bo[buffer].modifiable = false
  vim.api.nvim_set_current_buf(buffer)
end

local function validate_preview(contents)
  if type(contents) ~= "string" or #contents > MAX_PREVIEW_BYTES then
    return false, "preview output exceeds the size limit"
  end
  local decoded_ok, decoded = pcall(vim.json.decode, contents)
  if not decoded_ok or type(decoded) ~= "table" then
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
  if decoded.payload_schema_version ~= 1
    or decoded.purpose ~= PREVIEW_PURPOSE
    or type(decoded.summary) ~= "table"
    or decoded.summary.schema_version ~= 3
  then
    return false, "preview output has an unexpected format"
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
      if (string.sub(value, 1, 1) == "/" and value ~= "/")
        or string.match(value, "^%a:[/\\]")
        or (string.find(value, "/", 1, true) ~= nil
          and not safe_bracket_token)
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
    or (summary.schema_version ~= 1 and summary.schema_version ~= 2 and summary.schema_version ~= 3)
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

local function process_error(result)
  local detail = type(result.stderr) == "string" and result.stderr or ""
  detail = vim.trim(string.gsub(detail, "%s+", " "))
  detail = string.gsub(detail, "[%z\1-\31\127]", "?")
  if detail == "" then
    if type(result.signal) == "number" and result.signal ~= 0 then
      return "analyzer terminated by signal " .. tostring(result.signal)
    end
    return "analyzer exited with code " .. tostring(result.code)
  end
  if #detail > 512 then
    detail = string.sub(detail, 1, 509) .. "..."
  end
  return detail
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
    _generation = 0,
    _mkdir = deps.mkdir or vim.fn.mkdir,
    _notify_fn = deps.notify or default_notify,
    _open_file = deps.open_file or default_open_file,
    _open_preview = deps.open_preview or default_open_preview,
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
  local running = self._job ~= nil
  return { running = running, job = running and true or nil }
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
    self:_notify("report failed: " .. process_error(result), vim.log.levels.ERROR)
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
    self:_notify("preview failed: " .. process_error(result), vim.log.levels.ERROR)
    return
  end
  local valid, validation_error = validate_preview(result.stdout)
  if not valid then
    self:_notify(tostring(validation_error or "preview output is invalid"), vim.log.levels.ERROR)
    return
  end
  local open_ok, open_error = pcall(self._open_preview, result.stdout)
  if not open_ok then
    self:_notify("failed to open preview: " .. tostring(open_error), vim.log.levels.ERROR)
    return
  end
  self:_notify("sanitized Codex preview is ready", vim.log.levels.INFO)
end

function Report:start()
  if self._job ~= nil then
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
  if self._job ~= nil then
    self:_notify("a report or preview is already running", vim.log.levels.WARN)
    return false
  end
  local collect_ok, snapshot = pcall(self._collect_snapshot_payload)
  if not collect_ok or type(snapshot) ~= "string" or snapshot == "" then
    self:_notify("failed to collect keymap snapshot", vim.log.levels.ERROR)
    return false
  end
  local argv = {
    self._analyzer,
    "preview",
    self._summary_path,
    "--keymap-snapshot",
    "-",
    "--output",
    "-",
  }
  self._generation = self._generation + 1
  local generation = self._generation
  self._job = true
  local completed = false
  local run_ok, job = pcall(self._run, argv, function(result)
    completed = true
    self:_complete_preview(result, generation)
  end, snapshot)
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

function Report:shutdown()
  if self._job == nil then
    return false
  end
  self._generation = self._generation + 1
  local job = self._job
  self._job = nil
  if type(job) == "table" and type(job.kill) == "function" then
    pcall(job.kill, job, 15)
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
