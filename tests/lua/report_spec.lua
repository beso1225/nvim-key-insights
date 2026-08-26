local report = require("key-insights.report")
local process = require("key-insights.process")

local test_codex_home = vim.fn.tempname()
assert(vim.fn.mkdir(test_codex_home, "p", 448) >= 0)
local function isolated_codex_environment()
  return {
    CODEX_HOME = test_codex_home,
    PATH = "/usr/bin:/bin",
  }
end

local original_system = vim.system
local system_argv = nil
local system_options = nil
local system_result = nil
vim.system = function(argv, options, callback)
  system_argv = argv
  system_options = options
  callback({ code = 0 })
  return { pid = 7 }
end
process.run({ "tool with spaces", "arg;$" }, function(result)
  system_result = result
end)
vim.wait(1000, function()
  return system_result ~= nil
end)
vim.system = original_system
assert(vim.deep_equal(system_argv, { "tool with spaces", "arg;$" }))
assert(system_options.text == true)
assert(system_options.detach == true, "subprocesses must run in a dedicated process group")
assert(type(system_options.stdout) == "function")
assert(type(system_options.stderr) == "function")
assert(system_result.code == 0)

local isolated_system_options = nil
vim.system = function(_, options, callback)
  isolated_system_options = options
  callback({ code = 0 })
  return { pid = 8 }
end
process.run({ "isolated" }, function() end, nil, {
  clear_env = true,
  env = { KEY_INSIGHTS_ALLOWED = "allowed" },
})
vim.system = original_system
assert(isolated_system_options.clear_env == true)
local runtime_version = vim.version()
if runtime_version.major == 0 and runtime_version.minor == 10 then
  assert(vim.deep_equal(isolated_system_options.env, { "KEY_INSIGHTS_ALLOWED=allowed" }))
else
  assert(vim.deep_equal(isolated_system_options.env, { KEY_INSIGHTS_ALLOWED = "allowed" }))
end

vim.system = function(_, options, callback)
  isolated_system_options = options
  callback({ code = 0 })
  return { pid = 9 }
end
process.run({ "clear-only" }, function() end, nil, { clear_env = true })
vim.system = original_system
assert(isolated_system_options.clear_env == true)
if runtime_version.major == 0 and runtime_version.minor == 10 then
  assert(vim.deep_equal(isolated_system_options.env, {}))
else
  assert(isolated_system_options.env == nil)
end

local original_version = vim.version
vim.version = function()
  return { major = 0, minor = 10, patch = 4 }
end
vim.system = function(_, options, callback)
  isolated_system_options = options
  callback({ code = 0 })
  return { pid = 10 }
end
process.run({ "neovim-0.10-environment" }, function() end, nil, {
  clear_env = true,
  env = { PATH = "/usr/bin:/bin", CODEX_HOME = "/private/codex" },
})
vim.system = original_system
vim.version = original_version
assert(vim.deep_equal(isolated_system_options.env, {
  "CODEX_HOME=/private/codex",
  "PATH=/usr/bin:/bin",
}), "Neovim 0.10 requires a serialized clear environment")

if process.supports_process_groups() then
  local inherited = {
    KEY_INSIGHTS_UNRELATED_ENV_CANARY = "unrelated-environment-value-7f31",
    OPENAI_API_KEY = "api-key-environment-value-13a2",
    HTTPS_PROXY = "proxy-environment-value-4c18",
    SSL_CERT_FILE = "/private/custom-ca-environment-value-92df",
    DBUS_SESSION_BUS_ADDRESS = "dbus-environment-value-a741",
  }
  local previous_inherited = {}
  for name, value in pairs(inherited) do
    previous_inherited[name] = vim.env[name] or false
    vim.env[name] = value
  end
  local environment_result = nil
  process.run({ "/usr/bin/env" }, function(result)
    environment_result = result
  end, nil, {
    clear_env = true,
    env = { KEY_INSIGHTS_ALLOWED = "allowed" },
  })
  assert(vim.wait(1000, function()
    return environment_result ~= nil
  end), "isolated environment subprocess did not finish")
  for name, value in pairs(previous_inherited) do
    vim.env[name] = value == false and nil or value
  end
  assert(environment_result.code == 0)
  assert(string.find(environment_result.stdout, "KEY_INSIGHTS_ALLOWED=allowed", 1, true) ~= nil)
  for name, value in pairs(inherited) do
    assert(string.find(environment_result.stdout, name, 1, true) == nil)
    assert(string.find(environment_result.stdout, value, 1, true) == nil)
  end

  local shebang_directory = vim.fn.tempname()
  assert(vim.fn.mkdir(shebang_directory, "p", 448) >= 0)
  local shebang_script = vim.fs.joinpath(shebang_directory, "mock-codex")
  vim.fn.writefile({ "#!/usr/bin/env sh", "printf shebang-ok" }, shebang_script)
  assert(vim.uv.fs_chmod(shebang_script, 448))
  local shebang_result = nil
  process.run({ shebang_script }, function(result)
    shebang_result = result
  end, nil, {
    clear_env = true,
    env = { PATH = "/usr/bin:/bin" },
  })
  assert(vim.wait(1000, function()
    return shebang_result ~= nil
  end), "env shebang subprocess did not finish")
  assert(shebang_result.code == 0 and shebang_result.stdout == "shebang-ok")
  vim.fn.delete(shebang_directory, "rf")
end

local group_kills = {}
local group_callback = nil
local group_system = vim.system
local original_uv_kill = vim.uv.kill
vim.uv.kill = function(pid, signal)
  table.insert(group_kills, { pid = pid, signal = signal })
  return 0
end
vim.system = function(_, options, callback)
  assert(options.detach == true)
  group_callback = callback
  return { pid = 73, kill = function() error("direct kill must not be used on Unix") end }
end
local group_job = process.run({ "grouped" }, function() end, nil, { timeout_ms = math.huge })
group_job:kill(9)
assert(vim.deep_equal(group_kills, { { pid = -73, signal = 9 } }), "the entire process group must be killed")
group_callback({ code = 1, signal = 9 })
vim.wait(1000)
vim.system = group_system
vim.uv.kill = original_uv_kill

if process.supports_process_groups() then
  local descendant_marker = vim.fn.tempname()
  local descendant_result = nil
  process.run({
    "/bin/sh",
    "-c",
    "(trap '' TERM; sleep 1; printf leaked > \"$1\") </dev/null >/dev/null 2>/dev/null & exit 0",
    "holder",
    descendant_marker,
  }, function(result)
    descendant_result = result
  end, nil, { timeout_ms = 2000 })
  assert(vim.wait(1000, function()
    return descendant_result ~= nil
  end), "the direct child must complete promptly")
  vim.wait(1250)
  assert(vim.uv.fs_stat(descendant_marker) == nil, "successful direct exit must terminate descendants")
end

local bounded_result = nil
local bounded_system = vim.system
vim.system = function(_, options, callback)
  options.stdout(nil, string.rep("o", 1024 * 1024))
  options.stderr(nil, string.rep("e", 1024 * 1024))
  callback({ code = 0 })
  return { pid = 9 }
end
process.run({ "bounded" }, function(result)
  bounded_result = result
end)
vim.wait(1000, function()
  return bounded_result ~= nil
end)
vim.system = bounded_system
assert(#bounded_result.stdout == 256 * 1024 + 1, "stdout capture must be bounded")
assert(#bounded_result.stderr == 8 * 1024, "stderr capture must be bounded")

local expanded_result = nil
vim.system = function(_, options, callback)
  options.stdout(nil, string.rep("m", 512 * 1024))
  callback({ code = 0 })
  return { pid = 10 }
end
process.run({ "expanded" }, function(result)
  expanded_result = result
end, nil, { max_stdout_bytes = 1024 * 1024 + 1 })
vim.wait(1000, function()
  return expanded_result ~= nil
end)
vim.system = bounded_system
assert(#expanded_result.stdout == 512 * 1024, "renderer capture must use its separate bounded limit")

local timeout_result = nil
local timeout_system = vim.system
vim.system = function(_, _, callback)
  return {
    kill = function(_, signal)
      callback({ code = 1, signal = signal })
    end,
  }
end
process.run({ "hung" }, function(result)
  timeout_result = result
end, nil, { timeout_ms = 10 })
assert(vim.wait(1000, function()
  return timeout_result ~= nil
end), "process watchdog must terminate a hung subprocess")
assert(timeout_result.signal == 9)
vim.system = timeout_system

local notifications = {}
local invocations = {}
local invocation_stdin = {}
local opened = {}
local created_directories = {}
local protected_directories = {}
local pending_callback = nil
local outputs_valid = true
local snapshot_collections = 0
local workflow_events = {}

local instance = report.new({
  analyzer = "/tools/key insights;$analyzer",
  output_directory = "/state/key insights/reports;draft",
  session_directory = "/state/key insights/sessions;current",
}, {
  protect_directory = function(path, mode)
    table.insert(protected_directories, { path = path, mode = mode })
    return true
  end,
  mkdir = function(path, parents, mode)
    table.insert(created_directories, { path = path, parents = parents, mode = mode })
    return 1
  end,
  notify = function(message, level)
    table.insert(notifications, { message = message, level = level })
  end,
  open_file = function(path)
    table.insert(opened, path)
  end,
  collect_snapshot_payload = function()
    snapshot_collections = snapshot_collections + 1
    table.insert(workflow_events, "collect")
    return string.format('{"snapshot_version":1,"mappings":[],"marker":%d}', snapshot_collections)
  end,
  run = function(argv, callback, stdin)
    table.insert(workflow_events, "run")
    table.insert(invocations, vim.deepcopy(argv))
    table.insert(invocation_stdin, stdin)
    pending_callback = callback
    return { pid = 42 }
  end,
  validate_outputs = function()
    if outputs_valid then
      return true
    end
    return false, "the analyzer produced an invalid summary"
  end,
  validate_report = function()
    return true
  end,
})

assert(instance:start() == true)
assert(instance:status().running == true)
assert(instance:status().job ~= nil)
assert(instance:status().summary_path == nil, "status must not expose local output paths")
assert(vim.deep_equal(workflow_events, { "collect", "run" }), "snapshot collection must precede process launch")
assert(#created_directories == 1)
assert(created_directories[1].path == "/state/key insights/reports;draft")
assert(created_directories[1].parents == "p")
assert(created_directories[1].mode == 448, "report directories must be private")
assert(protected_directories[1].path == "/state/key insights/reports;draft")
assert(protected_directories[1].mode == 448)
assert(vim.deep_equal(invocations[1], {
  "/tools/key insights;$analyzer",
  "analyze",
  "--session-dir",
  "/state/key insights/sessions;current",
  "--summary",
  "/state/key insights/reports;draft/summary.json",
  "--report",
  "/state/key insights/reports;draft/report.md",
  "--keymap-snapshot",
  "-",
}), "report command must preserve argv without shell interpolation")
assert(invocation_stdin[1] == '{"snapshot_version":1,"mappings":[],"marker":1}')

assert(instance:start() == false, "a running report must reject concurrent invocation")
assert(#invocations == 1, "concurrent reports must not queue another process")
assert(snapshot_collections == 1, "concurrent reports must not recollect or mismatch the running snapshot")
assert(string.find(notifications[#notifications].message, "already running", 1, true) ~= nil)

pending_callback({ code = 0, signal = 0, stdout = "", stderr = "" })
assert(instance:status().running == false)
assert(#opened == 1)
assert(opened[1] == "/state/key insights/reports;draft/report.md")

outputs_valid = false
assert(instance:start() == true)
pending_callback({ code = 0, signal = 0, stdout = "", stderr = "" })
assert(#opened == 1, "malformed outputs must not be opened")
assert(string.find(notifications[#notifications].message, "invalid summary", 1, true) ~= nil)

outputs_valid = true
assert(instance:start() == true)
pending_callback({ code = 2, signal = 0, stdout = "", stderr = "invalid session\n" })
assert(#opened == 1, "failed analysis must preserve the current editor view")
assert(string.find(notifications[#notifications].message, "invalid session", 1, true) == nil)
assert(string.find(notifications[#notifications].message, "report failed", 1, true) ~= nil)

assert(instance:start() == true)
pending_callback({ code = 2, signal = 0, stdout = "", stderr = "\27[31munsafe\0message\n" })
assert(string.find(notifications[#notifications].message, "\27", 1, true) == nil, "notifications must drop controls")
assert(string.find(notifications[#notifications].message, "\0", 1, true) == nil, "notifications must drop NUL")

local missing_notifications = {}
local missing = report.new({
  analyzer = "missing-key-insights",
  output_directory = "/state/reports",
  session_directory = "/state/sessions",
}, {
  protect_directory = function()
    return true
  end,
  mkdir = function()
    return 1
  end,
  notify = function(message)
    table.insert(missing_notifications, message)
  end,
  collect_snapshot_payload = function()
    return '{"snapshot_version":1,"mappings":[]}'
  end,
  run = function()
    error("executable not found")
  end,
})
assert(missing:start() == false)
assert(missing:status().running == false)
assert(string.find(missing_notifications[1], "failed to start", 1, true) ~= nil)

local snapshot_failure_notifications = {}
local snapshot_failure_ran = false
local snapshot_failure = report.new({
  analyzer = "key-insights",
  output_directory = "/state/reports",
  session_directory = "/state/sessions",
}, {
  protect_directory = function()
    return true
  end,
  mkdir = function()
    return 1
  end,
  notify = function(message)
    table.insert(snapshot_failure_notifications, message)
  end,
  collect_snapshot_payload = function()
    return nil, "snapshot_payload:collection_failed secret-buffer-name"
  end,
  run = function()
    snapshot_failure_ran = true
  end,
})
assert(snapshot_failure:start() == false)
assert(snapshot_failure_ran == false, "snapshot failures must prevent analyzer launch")
assert(snapshot_failure:status().running == false)
assert(string.find(snapshot_failure_notifications[1], "failed to collect keymap snapshot", 1, true) ~= nil)
assert(string.find(snapshot_failure_notifications[1], "secret", 1, true) == nil, "snapshot errors must be content-free")

local shutdown_callback = nil
local shutdown_kills = 0
local shutdown_report = report.new({
  analyzer = "key-insights",
  output_directory = "/state/shutdown-reports",
  session_directory = "/state/sessions",
}, {
  protect_directory = function() return true end,
  mkdir = function() return 1 end,
  notify = function() end,
  collect_snapshot_payload = function()
    return '{"snapshot_version":1,"mappings":[]}'
  end,
  run = function(_, callback)
    shutdown_callback = callback
    return {
      kill = function(_, signal)
        assert(signal == 9)
        shutdown_kills = shutdown_kills + 1
      end,
    }
  end,
})
assert(shutdown_report:start() == true)
assert(shutdown_report:shutdown() == true)
assert(shutdown_report:status().running == false)
assert(shutdown_kills == 1)
shutdown_callback({ code = 0, signal = 0, stdout = "", stderr = "" })
assert(shutdown_report:status().running == false, "a late process callback must stay ignored")

local preview_notifications = {}
local preview_invocation = nil
local preview_stdin = nil
local preview_callback = nil
local shown_preview = nil
local valid_preview = vim.json.encode({
  payload_schema_version = 1,
  purpose = "analyze-neovim-usage",
  instructions = {
    action_kinds = { "learn_existing", "add_mapping", "change_mapping", "no_change" },
    evidence_required = true,
    collision_check_required = true,
    privacy_boundary = "Use only aggregate evidence and the optional sanitized keymap snapshot; do not request or infer raw input.",
  },
  summary = {
    schema_version = 3,
    ranking_limit = 100,
    sessions = 1,
    events = 1,
    total_session_duration_ms = 1,
    key_sequences = 0,
    sequence_keys = 0,
    text_runs = 0,
    text_keys = 0,
    mode_transitions = 0,
    mapping_uses = 0,
    repeated_key_runs = 0,
    repeated_key_presses = 0,
    unique_keys = 1,
    unique_mappings = 0,
    unique_repeated_keys = 0,
    modes = {},
    keys = { { key = "/", count = 1 } },
    mappings = {},
    repeated_keys = {},
    ergonomics = {
      contract_version = 1,
      candidate_limit = 100,
      thresholds = {
        minimum_candidate_sessions = 3,
        minimum_candidate_sequence_keys = 100,
        minimum_candidate_observations = 3,
      },
      distributions = {
        histogram_version = 1,
        session_duration_ms = {
          { bucket = "0-1s", count = 0 },
          { bucket = "1-10s", count = 0 },
          { bucket = "10-60s", count = 0 },
          { bucket = "1-5m", count = 0 },
          { bucket = "over-5m", count = 0 },
        },
        sequence_length_keys = {
          { bucket = "1", count = 0 },
          { bucket = "2", count = 0 },
          { bucket = "3-4", count = 0 },
          { bucket = "5-8", count = 0 },
          { bucket = "9-16", count = 0 },
          { bucket = "17-32", count = 0 },
          { bucket = "33-plus", count = 0 },
        },
        average_inter_key_latency_ms = {
          { bucket = "0-50ms", count = 0 },
          { bucket = "50-100ms", count = 0 },
          { bucket = "100-250ms", count = 0 },
          { bucket = "250-500ms", count = 0 },
          { bucket = "over-500ms", count = 0 },
        },
      },
      operations = { token_set_version = 1, undo = 0, redo = 0, ["repeat"] = 0, search_start = 0, search_navigation = 0 },
      count_prefixes = { token_set_version = 1, occurrences = 0, digit_presses = 0 },
      mode_transitions = {},
      repeated_motions = { token_set_version = 1, items = {} },
      mapping_coverage = { total_snapshot_mappings = 0, observed_mappings = 0, unobserved_mappings = 0 },
      candidates = {},
    },
  },
})
local default_openers = report.new({
  analyzer = "key-insights",
  output_directory = "/state/default-openers",
  session_directory = "/state/sessions",
}, { notify = function() end })
default_openers:_complete_preview({ code = 0, signal = 0, stdout = valid_preview, stderr = "" }, 0)
assert(vim.bo.filetype == "json", "the sanitized payload must open as JSON")
vim.api.nvim_buf_delete(0, { force = true })
default_openers._phase = "rendering_suggestions"
default_openers:_complete_suggestion_render({
  code = 0,
  signal = 0,
  stdout = "# Codex suggestions\n\nNo suggestions were returned.\n",
  stderr = "",
}, 0)
assert(vim.bo.filetype == "markdown", "deterministic suggestions must open as Markdown")
vim.api.nvim_buf_delete(0, { force = true })
local expanded_markdown_opened = false
local expanded_openers = report.new({
  analyzer = "key-insights",
  output_directory = "/state/expanded-openers",
  session_directory = "/state/sessions",
}, {
  notify = function() end,
  open_suggestions = function(contents)
    expanded_markdown_opened = #contents > 256 * 1024
  end,
})
expanded_openers._phase = "rendering_suggestions"
expanded_openers:_complete_suggestion_render({
  code = 0,
  signal = 0,
  stdout = "# Codex suggestions\n\n" .. string.rep("*", 300 * 1024),
  stderr = "",
}, 0)
assert(expanded_markdown_opened, "expanded valid Markdown must use its separate output bound")
local unsupported_notification = nil
local unsupported_codex = report.new({
  analyzer = "key-insights",
  output_directory = "/state/unsupported-codex-reports",
  session_directory = "/state/sessions",
}, {
  notify = function(message)
    unsupported_notification = message
  end,
  supports_process_groups = function()
    return false
  end,
  prepare_codex_directory = function()
    error("unsupported platforms must fail before preparing Codex")
  end,
  run_codex = function()
    error("unsupported platforms must never launch Codex")
  end,
})
assert(unsupported_codex:_start_codex(valid_preview, 0, vim.json.decode(valid_preview)) == false)
assert(string.find(unsupported_notification, "requires Unix process-group isolation", 1, true) ~= nil)

local unsupported_confirmed = false
local unsupported_resolved = false
local unsupported_analysis = report.new({
  analyzer = "key-insights",
  output_directory = "/state/unsupported-analysis-reports",
  session_directory = "/state/sessions",
}, {
  notify = function() end,
  supports_process_groups = function()
    return false
  end,
  resolve_codex_binary = function()
    unsupported_resolved = true
    return "/mock/codex"
  end,
  confirm = function()
    unsupported_confirmed = true
  end,
  open_preview = function() end,
  run = function(_, callback)
    callback({ code = 0, signal = 0, stdout = valid_preview, stderr = "" })
    return { pid = 79 }
  end,
})
assert(unsupported_analysis:analyze())
assert(not unsupported_resolved and not unsupported_confirmed)

local resolver_error_notification = nil
local resolver_error_analysis = report.new({
  analyzer = "key-insights",
  output_directory = "/state/resolver-error-reports",
  session_directory = "/state/sessions",
}, {
  notify = function(message)
    resolver_error_notification = message
  end,
  resolve_codex_binary = function()
    error("resolver detail must be contained")
  end,
  open_preview = function() end,
  run = function(_, callback)
    callback({ code = 0, signal = 0, stdout = valid_preview, stderr = "" })
    return { pid = 80 }
  end,
})
assert(resolver_error_analysis:analyze())
assert(resolver_error_analysis:status().running == false)
assert(string.find(resolver_error_notification, "failed to resolve", 1, true) ~= nil)
assert(string.find(resolver_error_notification, "resolver detail", 1, true) == nil)

local resolved_binary_directory = vim.fn.tempname()
assert(vim.fn.mkdir(resolved_binary_directory, "p", 448) >= 0)
local resolved_binary_path = vim.fs.joinpath(resolved_binary_directory, "mock-codex")
vim.fn.writefile({ "#!/bin/sh", "exit 0" }, resolved_binary_path)
assert(vim.uv.fs_chmod(resolved_binary_path, 448))
local previous_path = vim.env.PATH
vim.env.PATH = resolved_binary_directory .. ":" .. previous_path
local resolved_binary_argv = nil
local resolved_binary_report = report.new({
  analyzer = "key-insights",
  output_directory = "/state/resolved-binary-reports",
  session_directory = "/state/sessions",
  codex = { binary = "mock-codex" },
}, {
  notify = function() end,
  codex_environment = isolated_codex_environment,
  prepare_codex_directory = function()
    return true
  end,
  run_codex = function(argv)
    resolved_binary_argv = vim.deepcopy(argv)
    return { pid = 81, kill = function() end }
  end,
})
assert(resolved_binary_report:_start_codex(valid_preview, 0, vim.json.decode(valid_preview)))
assert(resolved_binary_argv[1] == resolved_binary_path)
assert(resolved_binary_report:shutdown())
vim.env.PATH = previous_path
vim.fn.delete(resolved_binary_directory, "rf")

local consent_directory = vim.fn.tempname()
assert(vim.fn.mkdir(consent_directory, "p", 448) >= 0)
local consent_binary = vim.fs.joinpath(consent_directory, "consented-codex")
vim.fn.writefile({ "#!/bin/sh", "exit 0" }, consent_binary)
assert(vim.uv.fs_chmod(consent_binary, 448))
local consent_previous_path = vim.env.PATH
vim.env.PATH = consent_directory .. ":" .. consent_previous_path
local consent_callback = nil
local consent_path = nil
local consent_launched = nil
local consent_environment = {
  CODEX_HOME = test_codex_home,
  PATH = consent_directory .. ":/usr/bin:/bin",
}
local consent_launch_options = nil
local consent_report = report.new({
  analyzer = "key-insights",
  output_directory = "/state/consent-reports",
  session_directory = "/state/sessions",
  codex = { binary = "consented-codex" },
}, {
  notify = function() end,
  open_preview = function() end,
  confirm = function(callback, binary)
    consent_callback = callback
    consent_path = binary
  end,
  codex_environment = function()
    return vim.deepcopy(consent_environment)
  end,
  prepare_codex_directory = function()
    return true
  end,
  run = function(_, callback)
    callback({ code = 0, signal = 0, stdout = valid_preview, stderr = "" })
    return { pid = 82 }
  end,
  run_codex = function(argv, _, _, options)
    consent_launched = argv[1]
    consent_launch_options = vim.deepcopy(options)
    return { pid = 83, kill = function() end }
  end,
})
assert(consent_report:analyze())
assert(consent_path == consent_binary)
vim.env.PATH = consent_previous_path
consent_environment.CODEX_HOME = consent_directory
consent_environment.PATH = "/changed/after/consent"
consent_callback(true)
assert(consent_launched == consent_binary)
assert(consent_launch_options.env.CODEX_HOME == test_codex_home)
assert(consent_launch_options.env.PATH == consent_directory .. ":/usr/bin:/bin")
assert(consent_report._resolved_codex_binary == nil)
assert(consent_report._resolved_codex_environment == nil)
assert(consent_report:shutdown())
vim.fn.delete(consent_directory, "rf")

local original_codex_home = vim.env.CODEX_HOME
local original_home = vim.env.HOME
local original_environment_path = vim.env.PATH
local environment_home = vim.fn.tempname()
local fallback_codex_home = vim.fs.joinpath(environment_home, ".codex")
assert(vim.fn.mkdir(fallback_codex_home, "p", 448) >= 0)
vim.env.CODEX_HOME = nil
vim.env.HOME = environment_home
vim.env.PATH = "/usr/bin:/bin"
local fallback_options = nil
local fallback_report = report.new({
  analyzer = "key-insights",
  output_directory = "/state/fallback-environment-reports",
  session_directory = "/state/sessions",
}, {
  notify = function() end,
  resolve_codex_binary = function()
    return "/mock/codex"
  end,
  prepare_codex_directory = function()
    return true
  end,
  run_codex = function(_, _, _, options)
    fallback_options = vim.deepcopy(options)
    return { pid = 84, kill = function() end }
  end,
})
assert(fallback_report:_start_codex(valid_preview, 0, vim.json.decode(valid_preview)))
assert(fallback_options.env.CODEX_HOME == fallback_codex_home)
assert(fallback_options.env.PATH == "/usr/bin:/bin")
assert(fallback_report:shutdown())

vim.env.CODEX_HOME = test_codex_home
local explicit_options = nil
local explicit_report = report.new({
  analyzer = "key-insights",
  output_directory = "/state/explicit-environment-reports",
  session_directory = "/state/sessions",
}, {
  notify = function() end,
  resolve_codex_binary = function()
    return "/mock/codex"
  end,
  prepare_codex_directory = function()
    return true
  end,
  run_codex = function(_, _, _, options)
    explicit_options = vim.deepcopy(options)
    return { pid = 85, kill = function() end }
  end,
})
assert(explicit_report:_start_codex(valid_preview, 0, vim.json.decode(valid_preview)))
assert(explicit_options.env.CODEX_HOME == test_codex_home)
assert(explicit_report:shutdown())

vim.env.CODEX_HOME = nil
vim.env.HOME = nil
local missing_home_notification = nil
local missing_home_report = report.new({
  analyzer = "key-insights",
  output_directory = "/state/missing-home-reports",
  session_directory = "/state/sessions",
}, {
  notify = function(message)
    missing_home_notification = message
  end,
  resolve_codex_binary = function()
    return "/mock/codex"
  end,
  prepare_codex_directory = function()
    return true
  end,
  run_codex = function()
    error("missing environment must not launch")
  end,
})
assert(missing_home_report:_start_codex(valid_preview, 0, vim.json.decode(valid_preview)) == false)
assert(string.find(missing_home_notification, "isolated Codex environment", 1, true) ~= nil)

vim.env.CODEX_HOME = vim.fs.joinpath(environment_home, "missing-codex-home")
vim.env.HOME = environment_home
assert(missing_home_report:_start_codex(valid_preview, 0, vim.json.decode(valid_preview)) == false)

local linked_codex_home = vim.fs.joinpath(environment_home, "linked-codex-home")
assert(vim.uv.fs_symlink(test_codex_home, linked_codex_home))
vim.env.CODEX_HOME = linked_codex_home
vim.env.HOME = environment_home
local live_invalid_launches = 0
local live_invalid_report = report.new({
  analyzer = "key-insights",
  output_directory = "/state/live-invalid-environment-reports",
  session_directory = "/state/sessions",
}, {
  notify = function() end,
  resolve_codex_binary = function()
    return "/mock/codex"
  end,
  prepare_codex_directory = function()
    return true
  end,
  run_codex = function()
    live_invalid_launches = live_invalid_launches + 1
  end,
})
assert(live_invalid_report:_start_codex(valid_preview, 0, vim.json.decode(valid_preview)) == false)
vim.env.CODEX_HOME = test_codex_home
vim.env.PATH = ""
assert(live_invalid_report:_start_codex(valid_preview, 0, vim.json.decode(valid_preview)) == false)
assert(live_invalid_launches == 0)
vim.env.CODEX_HOME = original_codex_home
vim.env.HOME = original_home
vim.env.PATH = original_environment_path
vim.fn.delete(environment_home, "rf")

local invalid_environment_notification = nil
local invalid_environment_codex = report.new({
  analyzer = "key-insights",
  output_directory = "/state/invalid-environment-reports",
  session_directory = "/state/sessions",
}, {
  notify = function(message)
    invalid_environment_notification = message
  end,
  codex_environment = function()
    return {
      CODEX_HOME = "relative-home",
      PATH = "/usr/bin:/bin",
      OPENAI_API_KEY = "must-not-cross",
    }
  end,
  resolve_codex_binary = function()
    return "/mock/codex"
  end,
  prepare_codex_directory = function()
    return true
  end,
  run_codex = function()
    error("invalid environments must never launch Codex")
  end,
})
assert(invalid_environment_codex:_start_codex(valid_preview, 0, vim.json.decode(valid_preview)) == false)
assert(string.find(invalid_environment_notification, "isolated Codex environment", 1, true) ~= nil)

local preview = report.new({
  analyzer = "/tools/key insights;$analyzer",
  output_directory = "/state/preview-reports",
  session_directory = "/state/sessions",
}, {
  notify = function(message)
    table.insert(preview_notifications, message)
  end,
  collect_snapshot_payload = function()
    error("a fresh preview must reconstruct an attributed report snapshot from summary.json")
  end,
  open_preview = function(payload)
    shown_preview = payload
  end,
  run = function(argv, callback, stdin)
    preview_invocation = vim.deepcopy(argv)
    preview_callback = callback
    preview_stdin = stdin
    return { pid = 43 }
  end,
})
assert(preview:preview() == true)
assert(vim.deep_equal(preview_invocation, {
  "/tools/key insights;$analyzer",
  "preview",
  "/state/preview-reports/summary.json",
  "--output",
  "-",
}))
assert(preview_stdin == nil)
assert(shown_preview == nil, "preview must wait for analyzer output")
preview_callback({
  code = 0,
  signal = 0,
  stdout = valid_preview,
  stderr = "",
})
assert(shown_preview == valid_preview)
assert(preview:status().running == false)

local reuse_snapshot_collections = 0
local reconstruction_callbacks = {}
local reconstruction_stdin = {}
local reconstruction_argv = {}
local snapshot_reconstruction = report.new({
  analyzer = "key-insights",
  output_directory = "/state/reused-snapshot-reports",
  session_directory = "/state/sessions",
}, {
  protect_directory = function()
    return true
  end,
  mkdir = function()
    return 1
  end,
  notify = function() end,
  open_file = function() end,
  collect_snapshot_payload = function()
    reuse_snapshot_collections = reuse_snapshot_collections + 1
    return string.format('{"snapshot_version":1,"mappings":[],"marker":%d}', reuse_snapshot_collections)
  end,
  run = function(argv, callback, stdin)
    table.insert(reconstruction_argv, vim.deepcopy(argv))
    table.insert(reconstruction_callbacks, callback)
    table.insert(reconstruction_stdin, stdin)
    return { pid = 49, kill = function() end }
  end,
  validate_outputs = function()
    return true
  end,
})
assert(snapshot_reconstruction:start() == true)
reconstruction_callbacks[1]({ code = 0, signal = 0, stdout = "", stderr = "" })
assert(snapshot_reconstruction:preview() == true)
assert(reuse_snapshot_collections == 1, "preview must not resample the current keymap")
assert(reconstruction_stdin[1] ~= nil and reconstruction_stdin[2] == nil)
assert(vim.tbl_contains(reconstruction_argv[2], "--keymap-snapshot") == false)
assert(snapshot_reconstruction:shutdown() == true)

local analysis_preview_callback = nil
local analysis_codex_callback = nil
local analysis_render_callback = nil
local analysis_render_stdin = nil
local analysis_render_options = nil
local analysis_codex_options = nil
local analysis_confirm_callback = nil
local analysis_confirm_binary = nil
local analysis_invocations = {}
local analysis_opened = {}
local analysis_markdown_opened = false
local analysis = report.new({
  analyzer = "key-insights",
  output_directory = "/state/analyze-reports",
  session_directory = "/state/sessions",
  codex = {
    binary = "/tools/codex;$",
    output_schema = "/state/schema.json",
    working_directory = "/state/empty-codex-workspace",
  },
}, {
  collect_snapshot_payload = function()
    return '{"snapshot_version":1,"mappings":[]}'
  end,
  open_preview = function(payload)
    table.insert(analysis_opened, payload)
  end,
  open_suggestions = function(payload)
    analysis_markdown_opened = true
    table.insert(analysis_opened, payload)
  end,
  confirm = function(callback, binary)
    analysis_confirm_callback = callback
    analysis_confirm_binary = binary
  end,
  prepare_codex_directory = function(path)
    assert(path == "/state/empty-codex-workspace")
    return true
  end,
  resolve_codex_binary = function(binary)
    return binary
  end,
  codex_environment = isolated_codex_environment,
  run = function(argv, callback)
    table.insert(analysis_invocations, vim.deepcopy(argv))
    analysis_preview_callback = callback
    return { pid = 50, kill = function() end }
  end,
  run_codex = function(argv, callback, stdin, options)
    table.insert(analysis_invocations, vim.deepcopy(argv))
    assert(stdin == shown_preview)
    analysis_codex_options = vim.deepcopy(options)
    analysis_codex_callback = callback
    return { pid = 51, kill = function() end }
  end,
  run_suggestions = function(argv, callback, stdin, options)
    assert(vim.deep_equal(argv, {
      "key-insights",
      "suggestions",
      "/state/analyze-reports/summary.json",
      "--input",
      "-",
      "--output",
      "-",
    }))
    analysis_render_callback = callback
    analysis_render_stdin = stdin
    analysis_render_options = options
    return { pid = 52, kill = function() end }
  end,
})
assert(analysis:analyze() == true)
analysis_preview_callback({ code = 0, signal = 0, stdout = shown_preview, stderr = "" })
assert(analysis:status().phase == "awaiting_confirmation")
assert(analysis_confirm_binary == "/tools/codex;$")
assert(#analysis_invocations == 1, "Codex must wait for explicit confirmation")
assert(analysis:preview() == false, "a pending confirmation must block another preview")
assert(analysis:start() == false, "a pending confirmation must block report generation")
analysis_confirm_callback(false)
assert(analysis:status().running == false)
assert(#analysis_invocations == 1, "cancelled analysis must not launch Codex")
assert(analysis:analyze() == true)
analysis_preview_callback({ code = 0, signal = 0, stdout = shown_preview, stderr = "" })
analysis_confirm_callback(true)
assert(analysis:status().phase == "codex")
assert(analysis_codex_options.clear_env == true)
assert(type(analysis_codex_options.env.CODEX_HOME) == "string")
assert(analysis_codex_options.env.CODEX_HOME ~= "")
assert(type(analysis_codex_options.env.PATH) == "string")
assert(analysis_codex_options.env.OPENAI_API_KEY == nil)
assert(vim.deep_equal(analysis_invocations[3], {
  "/tools/codex;$",
  "exec",
  "--ephemeral",
  "--ignore-user-config",
  "--ignore-rules",
  "--strict-config",
  "--skip-git-repo-check",
  "--cd",
  "/state/empty-codex-workspace",
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
  "/state/schema.json",
}))
analysis_codex_callback({
  code = 0,
  signal = 0,
  stdout = '{"schema_version":1,"suggestions":[{"action":"learn_existing","title":"Use / to search","rationale":"The measured search key is already available.","evidence":[{"metric":"sessions","value":1}],"collision_check":{"checked":true,"conflicting_mapping_ids":[]}}]}',
  stderr = "",
})
assert(analysis:status().phase == "rendering_suggestions")
assert(string.find(analysis_render_stdin, '"schema_version":1', 1, true) ~= nil)
assert(analysis_render_options.max_stdout_bytes == 1024 * 1024 + 1)
analysis_render_callback({
  code = 0,
  signal = 0,
  stdout = "# Codex suggestions\n\n## 1. Use the existing motion\n",
  stderr = "",
})
assert(analysis:status().running == false)
assert(#analysis_opened == 3, "preview and Codex output must be shown")
assert(analysis_markdown_opened == true)
assert(string.sub(analysis_opened[3], 1, 19) == "# Codex suggestions")

assert(analysis:analyze() == true)
analysis_preview_callback({ code = 0, signal = 0, stdout = shown_preview, stderr = "" })
assert(analysis:status().phase == "awaiting_confirmation")
assert(analysis:shutdown() == true, "shutdown must cancel a pending confirmation")
analysis_confirm_callback(true)
assert(#analysis_invocations == 4, "stale confirmation must not launch Codex")

assert(analysis:analyze() == true)
analysis_preview_callback({ code = 0, signal = 0, stdout = shown_preview, stderr = "" })
analysis_confirm_callback(true)
analysis_codex_callback({
  code = 0,
  signal = 0,
  stdout = '{"schema_version":1,"suggestions":[{"action":"learn_existing","title":"Use the existing motion","rationale":"The measured motion is already available.","evidence":[{"metric":"sessions","value":999}],"collision_check":{"checked":true,"conflicting_mapping_ids":[]}}]}',
  stderr = "",
})
assert(analysis:status().running == false)
assert(#analysis_opened == 5, "invalid Codex output must not be opened")

assert(analysis:analyze() == true)
analysis_preview_callback({ code = 0, signal = 0, stdout = shown_preview, stderr = "" })
analysis_confirm_callback(true)
analysis_codex_callback({
  code = 0,
  signal = 0,
  stdout = '{"schema_version":1,"suggestions":[{"action":"learn_existing","title":"Mislabel a histogram","rationale":"Histogram bucket counts are not scalar duration measurements.","evidence":[{"metric":"session_duration_ms","value":1}],"collision_check":{"checked":true,"conflicting_mapping_ids":[]}}]}',
  stderr = "",
})
assert(analysis:status().running == false)
assert(#analysis_opened == 6, "histogram names must not be accepted as scalar evidence")

assert(analysis:analyze() == true)
analysis_preview_callback({ code = 0, signal = 0, stdout = shown_preview, stderr = "" })
analysis_confirm_callback(true)
analysis_codex_callback({
  code = 0,
  signal = 0,
  stdout = '{"schema_version":1,"suggestions":[{"action":"learn_existing","title":"Review src/config.lua","rationale":"The measured search key is already available.","evidence":[{"metric":"sessions","value":1}],"collision_check":{"checked":true,"conflicting_mapping_ids":[]}}]}',
  stderr = "",
})
assert(analysis:status().running == false)
assert(#analysis_opened == 7, "path-shaped Codex output must not be opened")

assert(analysis:analyze() == true)
analysis_preview_callback({ code = 0, signal = 0, stdout = shown_preview, stderr = "" })
analysis_confirm_callback(true)
analysis_codex_callback({
  code = 0,
  signal = 0,
  stdout = '{"schema_version":1,"schema_\\u0076ersion":1,"suggestions":[]}',
  stderr = "",
})
assert(analysis:status().running == false)
assert(#analysis_opened == 8, "duplicate JSON keys must be rejected before opening Codex output")

assert(analysis:analyze() == true)
analysis_preview_callback({ code = 0, signal = 0, stdout = shown_preview, stderr = "" })
analysis_confirm_callback(true)
analysis_codex_callback({
  code = 0,
  signal = 0,
  stdout = '{"schema_version":1,"suggestions":[{"action":"no_change","title":"Keep the current setup","rationale":"The measured sample does not justify a change.","evidence":[{"metric":"sessions","value":1}],"collision_check":{"checked":true,"conflicting_mapping_ids":[]}}]}',
  stderr = "",
})
assert(analysis:status().phase == "rendering_suggestions")
analysis_render_callback({ code = 1, signal = 0, stdout = "raw model response", stderr = "/secret" })
assert(analysis:status().running == false)
assert(#analysis_opened == 9, "renderer failure must not open raw or partial output")

local global_gg = "mapping-v1:a27261baf28b456378725590385ed469ee8c2c2e3fd5173cd32c7dbec271cc71"
local prefix_preview_table = vim.json.decode(valid_preview)
prefix_preview_table.keymap_snapshot = {
  snapshot_version = 1,
  mappings = { { mapping_id = global_gg, mode = "normal", scope = "global", lhs = { "g", "g" } } },
}
prefix_preview_table.summary.ergonomics.mapping_coverage = {
  snapshot_version = 1,
  total_snapshot_mappings = 1,
  observed_mappings = 0,
  unobserved_mappings = 1,
}
prefix_preview_table.summary.mapping_attribution = {
  snapshot_version = 1,
  mappings = {
    {
      mapping_id = global_gg,
      status = "unobserved_in_sample",
      count = 0,
      mode = "normal",
      scope = "global",
      lhs = { "g", "g" },
    },
  },
  collisions = {},
}
local prefix_preview = vim.json.encode(prefix_preview_table)
local function run_prefix_case(conflicting_mapping_ids, should_render, proposal_lhs, preview_payload)
  local opened_outputs = {}
  local rendered = false
  local notifications = {}
  local prefix_case = report.new({
    analyzer = "key-insights",
    output_directory = "/state/prefix-reports",
    session_directory = "/state/sessions",
  }, {
    notify = function(message)
      table.insert(notifications, message)
    end,
    open_preview = function(contents)
      table.insert(opened_outputs, contents)
    end,
    confirm = function(callback)
      callback(true)
    end,
    prepare_codex_directory = function()
      return true
    end,
    codex_environment = isolated_codex_environment,
    resolve_codex_binary = function()
      return "/mock/codex"
    end,
    run = function(_, callback)
      callback({ code = 0, signal = 0, stdout = preview_payload or prefix_preview, stderr = "" })
      return { pid = 60 }
    end,
    run_codex = function(_, callback)
      callback({
        code = 0,
        signal = 0,
        stdout = vim.json.encode({
          schema_version = 1,
          suggestions = {
            {
              action = "add_mapping",
              title = "Add a shorter mapping",
              rationale = "The measured sample supports considering the shorter prefix.",
              mapping = { mode = "normal", scope = "global", lhs = proposal_lhs or { "g" } },
              evidence = { { metric = "sessions", value = 1 } },
              collision_check = { checked = true, conflicting_mapping_ids = conflicting_mapping_ids },
            },
          },
        }),
        stderr = "",
      })
      return { pid = 61 }
    end,
    run_suggestions = function(_, callback)
      rendered = true
      callback({ code = 0, signal = 0, stdout = "# Codex suggestions\n\n", stderr = "" })
      return { pid = 62 }
    end,
  })
  assert(prefix_case:analyze() == true)
  assert(rendered == should_render)
  return opened_outputs, notifications
end

local prefix_blind_outputs, prefix_blind_notifications = run_prefix_case({}, false)
assert(#prefix_blind_outputs == 1, "a prefix-blind proposal must not be rendered")
local prefix_omission_reported = false
for _, message in ipairs(prefix_blind_notifications) do
  prefix_omission_reported = prefix_omission_reported or string.find(message, "omitted", 1, true) ~= nil
end
assert(prefix_omission_reported)
local prefix_checked_outputs = run_prefix_case({ global_gg }, true)
assert(#prefix_checked_outputs == 2)
assert(prefix_checked_outputs[2] == "# Codex suggestions\n\n")

local global_g = "mapping-v1:494845698ff45708f6996ca041b292cbe37a38c30e46af662058ec44d0ba2e67"
local shorter_preview_table = vim.deepcopy(prefix_preview_table)
shorter_preview_table.keymap_snapshot.mappings[1].mapping_id = global_g
shorter_preview_table.keymap_snapshot.mappings[1].lhs = { "g" }
shorter_preview_table.summary.mapping_attribution.mappings[1].mapping_id = global_g
shorter_preview_table.summary.mapping_attribution.mappings[1].lhs = { "g" }
local shorter_preview = vim.json.encode(shorter_preview_table)
local reverse_prefix_outputs = run_prefix_case({ global_g }, true, { "g", "g" }, shorter_preview)
assert(#reverse_prefix_outputs == 2, "a longer proposal must report the existing shorter mapping")

local forged_preview = report.new({
  analyzer = "key-insights",
  output_directory = "/state/preview-reports",
  session_directory = "/state/sessions",
}, {
  notify = function(message)
    table.insert(preview_notifications, message)
  end,
  collect_snapshot_payload = function()
    return '{"snapshot_version":1,"mappings":[]}'
  end,
  open_preview = function()
    error("forged preview must not be opened")
  end,
  run = function(_, callback)
    local forged = vim.json.decode(valid_preview)
    forged.summary.path = "/Users/secret"
    callback({
      code = 0,
      signal = 0,
      stdout = vim.json.encode(forged),
      stderr = "",
    })
    return { pid = 45 }
  end,
})
assert(forged_preview:preview() == true)
assert(string.find(preview_notifications[#preview_notifications], "forbidden field", 1, true) ~= nil)

local nested_forged_key = "<file:///home/alice/project>"
local nested_extra_field = false
local nested_forged_preview = report.new({
  analyzer = "key-insights",
  output_directory = "/state/preview-reports",
  session_directory = "/state/sessions",
}, {
  notify = function(message)
    table.insert(preview_notifications, message)
  end,
  collect_snapshot_payload = function()
    return '{"snapshot_version":1,"mappings":[]}'
  end,
  open_preview = function()
    error("nested forged preview must not be opened")
  end,
  run = function(_, callback)
    local forged = vim.json.decode(valid_preview)
    forged.summary.keys = { { key = nested_forged_key, count = 1 } }
    if nested_extra_field then
      forged.summary.keys[1].mode = "normal"
    end
    callback({
      code = 0,
      signal = 0,
      stdout = vim.json.encode(forged),
      stderr = "",
    })
    return { pid = 46 }
  end,
})
assert(nested_forged_preview:preview() == true)
assert(string.find(preview_notifications[#preview_notifications], "unexpected format", 1, true) ~= nil)
nested_forged_key = "hunter2"
assert(nested_forged_preview:preview() == true)
assert(
  string.find(preview_notifications[#preview_notifications], "unexpected format", 1, true) ~= nil,
  "non-canonical summary tokens must be rejected before Codex"
)
nested_forged_key = "/"
nested_extra_field = true
assert(nested_forged_preview:preview() == true)
assert(
  string.find(preview_notifications[#preview_notifications], "unexpected format", 1, true) ~= nil,
  "known fields in the wrong nested object must be rejected"
)
nested_extra_field = false
local inexact_counter_preview = report.new({
  analyzer = "key-insights",
  output_directory = "/state/preview-reports",
  session_directory = "/state/sessions",
}, {
  notify = function(message)
    table.insert(preview_notifications, message)
  end,
  open_preview = function()
    error("inexact JSON counters must not be opened")
  end,
  run = function(_, callback)
    local forged = vim.json.decode(valid_preview)
    forged.summary.sessions = 9007199254740992
    callback({ code = 0, signal = 0, stdout = vim.json.encode(forged), stderr = "" })
    return { pid = 46 }
  end,
})
assert(inexact_counter_preview:preview() == true)
assert(string.find(preview_notifications[#preview_notifications], "unexpected format", 1, true) ~= nil)

local malformed_attribution = report.new({
  analyzer = "key-insights",
  output_directory = "/state/preview-reports",
  session_directory = "/state/sessions",
}, {
  notify = function(message)
    table.insert(preview_notifications, message)
  end,
  collect_snapshot_payload = function()
    return '{"snapshot_version":1,"mappings":[]}'
  end,
  open_preview = function()
    error("malformed attribution must not be opened")
  end,
  run = function(_, callback)
    local forged = vim.json.decode(valid_preview)
    forged.keymap_snapshot = { snapshot_version = 1, mappings = {} }
    forged.summary.mapping_attribution = {}
    callback({ code = 0, signal = 0, stdout = vim.json.encode(forged), stderr = "" })
    return { pid = 47 }
  end,
})
assert(malformed_attribution:preview() == true)
assert(string.find(preview_notifications[#preview_notifications], "keymap snapshot", 1, true) ~= nil)

local oversized_preview = report.new({
  analyzer = "key-insights",
  output_directory = "/state/preview-reports",
  session_directory = "/state/sessions",
}, {
  notify = function(message)
    table.insert(preview_notifications, message)
  end,
  collect_snapshot_payload = function()
    return '{"snapshot_version":1,"mappings":[]}'
  end,
  run = function(_, callback)
    callback({ code = 0, signal = 0, stdout = string.rep("x", 262145), stderr = "" })
    return { pid = 44 }
  end,
})
assert(oversized_preview:preview() == true)
assert(string.find(preview_notifications[#preview_notifications], "preview", 1, true) ~= nil)

assert(instance:open() == true)
assert(#opened == 2)

local real_directory = vim.fn.tempname()
local real_opened = 0
local write_outputs = true
local real_notifications = {}
local real = report.new({
  analyzer = "key-insights",
  output_directory = real_directory,
  session_directory = real_directory .. "/sessions",
}, {
  notify = function(message)
    table.insert(real_notifications, message)
  end,
  open_file = function()
    real_opened = real_opened + 1
  end,
  run = function(_, callback)
    if write_outputs then
      vim.fn.writefile({ '{"schema_version":1,"sessions":1,"events":2}' }, real_directory .. "/summary.json")
      vim.fn.writefile({ "# Neovim Key Insights", "", "report" }, real_directory .. "/report.md")
    end
    callback({ code = 0, stdout = "", stderr = "" })
    return { pid = 8 }
  end,
})
assert(real:start() == true)
assert(real_opened == 1, "fresh validated outputs must open")
write_outputs = false
assert(real:start() == true)
assert(real_opened == 1, "stale outputs must not be accepted as a fresh report")
assert(string.find(real_notifications[#real_notifications], "fresh outputs", 1, true) ~= nil)
vim.fn.delete(real_directory, "rf")

local symlink_root = vim.fn.tempname()
local symlink_target = symlink_root .. "/target"
local symlink_output = symlink_root .. "/reports"
vim.fn.mkdir(symlink_target, "p", 493)
assert(vim.uv.fs_chmod(symlink_target, 493))
assert(vim.uv.fs_symlink(symlink_target, symlink_output))
local symlink_ran = false
local symlink_report = report.new({
  analyzer = "key-insights",
  output_directory = symlink_output,
  session_directory = symlink_root .. "/sessions",
}, {
  notify = function() end,
  run = function()
    symlink_ran = true
  end,
})
assert(symlink_report:start() == false, "report output directories must not follow symlinks")
assert(symlink_ran == false)
assert(vim.uv.fs_stat(symlink_target).mode % 512 == 493, "a symlink target's mode must remain unchanged")
vim.fn.delete(symlink_root, "rf")

local report_open_flags = nil
local nonblocking_fs = {
  fs_lstat = function()
    return {
      dev = 1,
      ino = 2,
      mode = 384,
      mtime = { nsec = 0, sec = 1 },
      nlink = 1,
      size = 32,
      type = "file",
      uid = 1000,
    }
  end,
  fs_open = function(_, flags)
    report_open_flags = flags
    return nil, "ENOENT: injected replacement"
  end,
}
local nonblocking_report = report.new({
  analyzer = "key-insights",
  output_directory = "/unused/reports",
  session_directory = "/unused/sessions",
}, {
  fs = nonblocking_fs,
  notify = function() end,
})
assert(nonblocking_report:open() == false)
assert(
  report_open_flags == vim.uv.constants.O_RDONLY + vim.uv.constants.O_NONBLOCK,
  "report reads must not block on a replaced FIFO"
)

print("Lua report workflow contract: ok")
vim.fn.delete(test_codex_home, "rf")
