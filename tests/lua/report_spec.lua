local report = require("key-insights.report")
local process = require("key-insights.process")

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
assert(type(system_options.stdout) == "function")
assert(type(system_options.stderr) == "function")
assert(system_result.code == 0)

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
assert(timeout_result.signal == 15)
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
assert(string.find(notifications[#notifications].message, "invalid session", 1, true) ~= nil)

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
        assert(signal == 15)
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
local preview = report.new({
  analyzer = "/tools/key insights;$analyzer",
  output_directory = "/state/preview-reports",
  session_directory = "/state/sessions",
}, {
  notify = function(message)
    table.insert(preview_notifications, message)
  end,
  collect_snapshot_payload = function()
    return '{"snapshot_version":1,"mappings":[]}'
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
  "--keymap-snapshot",
  "-",
  "--output",
  "-",
}))
assert(preview_stdin == '{"snapshot_version":1,"mappings":[]}')
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
local reuse_snapshot_callbacks = {}
local reuse_snapshot_stdin = {}
local snapshot_reuse = report.new({
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
  run = function(_, callback, stdin)
    table.insert(reuse_snapshot_callbacks, callback)
    table.insert(reuse_snapshot_stdin, stdin)
    return { pid = 49, kill = function() end }
  end,
  validate_outputs = function()
    return true
  end,
})
assert(snapshot_reuse:start() == true)
reuse_snapshot_callbacks[1]({ code = 0, signal = 0, stdout = "", stderr = "" })
assert(snapshot_reuse:preview() == true)
assert(reuse_snapshot_collections == 1, "preview must reuse the report-time snapshot")
assert(reuse_snapshot_stdin[2] == reuse_snapshot_stdin[1], "summary and preview snapshots must stay identical")
assert(snapshot_reuse:shutdown() == true)

local analysis_preview_callback = nil
local analysis_codex_callback = nil
local analysis_confirm_callback = nil
local analysis_invocations = {}
local analysis_opened = {}
local analysis = report.new({
  analyzer = "key-insights",
  output_directory = "/state/analyze-reports",
  session_directory = "/state/sessions",
  codex = { binary = "/tools/codex;$", output_schema = "/state/schema.json" },
}, {
  collect_snapshot_payload = function()
    return '{"snapshot_version":1,"mappings":[]}'
  end,
  open_preview = function(payload)
    table.insert(analysis_opened, payload)
  end,
  confirm = function(callback)
    analysis_confirm_callback = callback
  end,
  run = function(argv, callback)
    table.insert(analysis_invocations, vim.deepcopy(argv))
    analysis_preview_callback = callback
    return { pid = 50, kill = function() end }
  end,
  run_codex = function(argv, callback, stdin)
    table.insert(analysis_invocations, vim.deepcopy(argv))
    assert(stdin == shown_preview)
    analysis_codex_callback = callback
    return { pid = 51, kill = function() end }
  end,
})
assert(analysis:analyze() == true)
analysis_preview_callback({ code = 0, signal = 0, stdout = shown_preview, stderr = "" })
assert(analysis:status().phase == "awaiting_confirmation")
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
assert(vim.deep_equal(analysis_invocations[3], {
  "/tools/codex;$",
  "exec",
  "--ephemeral",
  "--sandbox",
  "read-only",
  "--output-schema",
  "/state/schema.json",
}))
analysis_codex_callback({
  code = 0,
  signal = 0,
  stdout = '{"schema_version":1,"suggestions":[{"action":"learn_existing","title":"Use / to search","rationale":"The measured search key is already available.","evidence":[{"metric":"sessions","value":1}],"collision_check":{"checked":true,"conflicting_mapping_ids":[]}}]}',
  stderr = "",
})
assert(analysis:status().running == false)
assert(#analysis_opened == 3, "preview and Codex output must be shown")

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
  stdout = '{"schema_version":1,"suggestions":[{"action":"learn_existing","title":"Review src/config.lua","rationale":"The measured search key is already available.","evidence":[{"metric":"sessions","value":1}],"collision_check":{"checked":true,"conflicting_mapping_ids":[]}}]}',
  stderr = "",
})
assert(analysis:status().running == false)
assert(#analysis_opened == 6, "path-shaped Codex output must not be opened")

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
    forged.summary.keys = { { key = "<file:///home/alice/project>", count = 1 } }
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
assert(string.find(preview_notifications[#preview_notifications], "forbidden field", 1, true) ~= nil)

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
