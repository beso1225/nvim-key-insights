local filesystem = require("key-insights.filesystem")
local process = require("key-insights.process")
local snapshot_publisher = require("key-insights.snapshot_publisher")

local M = {}
local Report = {}
Report.__index = Report

local OWNER_DIRECTORY = 448 -- 0700
local MAX_SUMMARY_BYTES = 4 * 1024 * 1024
local MAX_REPORT_BYTES = 1024 * 1024
local REPORT_HEADER = "# Neovim Key Insights"

local function default_notify(message, level)
  vim.notify("key-insights: " .. message, level)
end

local function default_open_file(path)
  vim.api.nvim_cmd({ cmd = "edit", args = { path } }, {})
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
    or summary.schema_version ~= 1
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
  local publisher = nil
  if deps.publish_snapshot == nil then
    publisher = snapshot_publisher.new({
      collector_options = config.collector_options,
      output_directory = config.output_directory,
    }, { fs = fs })
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
    _output_directory = config.output_directory,
    _previous_outputs = nil,
    _protect_directory = deps.protect_directory or function(path, mode)
      return protect_directory(fs, path, mode)
    end,
    _publish_snapshot = deps.publish_snapshot or function()
      return publisher:publish()
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
  local publish_ok, snapshot_path, snapshot_identity = pcall(self._publish_snapshot)
  if not publish_ok or type(snapshot_path) ~= "string" or snapshot_path == "" then
    self._previous_outputs = nil
    self:_notify("failed to publish keymap snapshot", vim.log.levels.ERROR)
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
    snapshot_path,
  }
  if type(snapshot_identity) == "string" and snapshot_identity ~= "" then
    table.insert(argv, "--keymap-snapshot-identity")
    table.insert(argv, snapshot_identity)
  end
  self._generation = self._generation + 1
  local generation = self._generation
  self._job = true
  local completed = false
  local run_ok, job = pcall(self._run, argv, function(result)
    completed = true
    self:_complete(result, generation)
  end)
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
