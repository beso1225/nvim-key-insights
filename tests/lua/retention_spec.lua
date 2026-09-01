local artifacts = require("key-insights.artifacts")
local config = require("key-insights.config")
local filesystem = require("key-insights.filesystem")
local schema = require("key-insights.schema")
local storage = require("key-insights.storage")

local DAY_SECONDS = 24 * 60 * 60
local NOW_SECONDS = 2000000000
local FILE_PREFIX = "nvim-key-insights-"

local function log_name(session_id)
  return FILE_PREFIX .. session_id .. ".jsonl"
end

local function partial_name(session_id)
  return FILE_PREFIX .. session_id .. ".jsonl.part"
end

local function lock_name(session_id)
  return FILE_PREFIX .. session_id .. ".lock"
end

local function write_at(path, contents, modified_at)
  vim.fn.writefile({ contents }, path)
  assert(vim.uv.fs_chmod(path, 384))
  local ok, error_message = vim.uv.fs_utime(path, modified_at, modified_at)
  assert(ok, error_message)
end

local function basenames(pattern)
  local names = vim.tbl_map(vim.fs.basename, vim.fn.glob(pattern, false, true))
  table.sort(names)
  return names
end

local function finalize(store, session_id)
  local session = store:open_session(session_id)
  session:write({
    schema.encode(schema.session_start(session_id)),
    schema.encode(schema.session_end(session_id, 1)),
  })
  session:finish()
end

local defaults = config.defaults()
assert(defaults.storage.retention.max_sessions == 100)
assert(defaults.storage.retention.max_age_days == 30)

for _, invalid in ipairs({
  { max_sessions = 0 },
  { max_sessions = 1.5 },
  { max_sessions = math.huge },
  { max_age_days = 0 },
  { max_age_days = 1.5 },
  { max_age_days = math.huge },
}) do
  assert(pcall(function()
    config.resolve({ storage = { retention = invalid } })
  end) == false, "invalid retention values must be rejected")
end

local directory = vim.fn.tempname()
vim.fn.mkdir(directory, "p", 448)
write_at(vim.fs.joinpath(directory, log_name("expired")), "expired", NOW_SECONDS - 31 * DAY_SECONDS)
write_at(vim.fs.joinpath(directory, log_name("count-old")), "count-old", NOW_SECONDS - 10 * DAY_SECONDS)
write_at(vim.fs.joinpath(directory, log_name("recent-a")), "recent-a", NOW_SECONDS - 2 * DAY_SECONDS)
write_at(vim.fs.joinpath(directory, log_name("recent-b")), "recent-b", NOW_SECONDS - DAY_SECONDS)
write_at(vim.fs.joinpath(directory, log_name("boundary")), "boundary", NOW_SECONDS - 30 * DAY_SECONDS)
write_at(vim.fs.joinpath(directory, partial_name("crashed")), "partial", NOW_SECONDS - 90 * DAY_SECONDS)
write_at(vim.fs.joinpath(directory, lock_name("crashed")), "lock", NOW_SECONDS - 90 * DAY_SECONDS)
write_at(vim.fs.joinpath(directory, "notes.jsonl.backup"), "unrelated", NOW_SECONDS - 90 * DAY_SECONDS)
write_at(vim.fs.joinpath(directory, "report.jsonl"), "unrelated", NOW_SECONDS - 90 * DAY_SECONDS)
local custom_legacy_name = string.rep("f", 32) .. ".jsonl"
write_at(vim.fs.joinpath(directory, custom_legacy_name), "unrelated", NOW_SECONDS - 90 * DAY_SECONDS)

local store = storage.new({
  directory = directory,
  now_seconds = function()
    return NOW_SECONDS
  end,
  retention = {
    max_age_days = 30,
    max_sessions = 4,
  },
})
finalize(store, "current")

local retained_names = basenames(directory .. "/*.jsonl")
assert(vim.deep_equal(retained_names, {
  custom_legacy_name,
  log_name("count-old"),
  log_name("current"),
  log_name("recent-a"),
  log_name("recent-b"),
  "report.jsonl",
}), "retention must remove expired logs before pruning the oldest excess sessions: " .. vim.inspect(retained_names))
for _, protected_name in ipairs({
  partial_name("crashed"),
  lock_name("crashed"),
  "notes.jsonl.backup",
  "report.jsonl",
  custom_legacy_name,
}) do
  assert(vim.uv.fs_stat(vim.fs.joinpath(directory, protected_name)) ~= nil, protected_name .. " must not be pruned")
end
vim.fn.delete(directory, "rf")

local legacy_directory = vim.fn.tempname()
vim.fn.mkdir(legacy_directory, "p", 448)
local expired_legacy_name = string.rep("a", 32) .. ".jsonl"
local excess_legacy_name = string.rep("b", 32) .. ".jsonl"
write_at(vim.fs.joinpath(legacy_directory, expired_legacy_name), "expired", NOW_SECONDS - 31 * DAY_SECONDS)
write_at(vim.fs.joinpath(legacy_directory, excess_legacy_name), "excess", NOW_SECONDS - DAY_SECONDS)
local legacy_store = storage.new({
  default_directory = function()
    return legacy_directory
  end,
  now_seconds = function()
    return NOW_SECONDS
  end,
  retention = {
    max_age_days = 30,
    max_sessions = 1,
  },
})
finalize(legacy_store, "current")
assert(vim.deep_equal(basenames(legacy_directory .. "/*.jsonl"), {
  log_name("current"),
}), "the default collector directory must continue pruning pre-namespace logs")
vim.fn.delete(legacy_directory, "rf")

local boundary_directory = vim.fn.tempname()
vim.fn.mkdir(boundary_directory, "p", 448)
write_at(vim.fs.joinpath(boundary_directory, log_name("boundary")), "boundary", NOW_SECONDS - 30 * DAY_SECONDS)
local boundary_store = storage.new({
  directory = boundary_directory,
  now_seconds = function()
    return NOW_SECONDS
  end,
  retention = {
    max_age_days = 30,
    max_sessions = 2,
  },
})
finalize(boundary_store, "current")
assert(vim.deep_equal(basenames(boundary_directory .. "/*.jsonl"), {
  log_name("boundary"),
  log_name("current"),
}), "a log exactly at the age boundary must be retained")
vim.fn.delete(boundary_directory, "rf")

local locked_directory = vim.fn.tempname()
vim.fn.mkdir(locked_directory, "p", 448)
write_at(vim.fs.joinpath(locked_directory, log_name("active")), "active", NOW_SECONDS - 90 * DAY_SECONDS)
write_at(
  vim.fs.joinpath(locked_directory, lock_name("active")),
  vim.json.encode({ pid = 123, version = 1 }),
  NOW_SECONDS - 90 * DAY_SECONDS
)
write_at(vim.fs.joinpath(locked_directory, log_name("stale")), "stale", NOW_SECONDS - 90 * DAY_SECONDS)
write_at(
  vim.fs.joinpath(locked_directory, lock_name("stale")),
  vim.json.encode({ pid = 456, version = 1 }),
  NOW_SECONDS - 90 * DAY_SECONDS
)
write_at(vim.fs.joinpath(locked_directory, log_name("malformed")), "malformed", NOW_SECONDS - 90 * DAY_SECONDS)
write_at(vim.fs.joinpath(locked_directory, lock_name("malformed")), "not-json", NOW_SECONDS - 90 * DAY_SECONDS)
local active_process_alive = true
local locked_store = storage.new({
  directory = locked_directory,
  is_process_alive = function(pid)
    return active_process_alive and pid == 123
  end,
  now_seconds = function()
    return NOW_SECONDS
  end,
  retention = {
    max_age_days = 30,
    max_sessions = 1,
  },
})
finalize(locked_store, "current")
assert(vim.deep_equal(basenames(locked_directory .. "/*.jsonl"), {
  log_name("active"),
  log_name("current"),
}), "a finalized log with an active lock must remain protected")
active_process_alive = false
finalize(locked_store, "later")
assert(vim.deep_equal(basenames(locked_directory .. "/*.jsonl"), { log_name("later") }))
vim.fn.delete(locked_directory, "rf")

local unknown_type_directory = vim.fn.tempname()
vim.fn.mkdir(unknown_type_directory, "p", 448)
write_at(vim.fs.joinpath(unknown_type_directory, log_name("old")), "old", NOW_SECONDS - 90 * DAY_SECONDS)
local unknown_type_fs = setmetatable({}, { __index = vim.uv })
unknown_type_fs.fs_scandir_next = function(request)
  local name = vim.uv.fs_scandir_next(request)
  return name, nil
end
local unknown_type_store = storage.new({
  directory = unknown_type_directory,
  fs = unknown_type_fs,
  now_seconds = function()
    return NOW_SECONDS
  end,
  retention = {
    max_age_days = 30,
    max_sessions = 1,
  },
})
finalize(unknown_type_store, "current")
assert(vim.deep_equal(basenames(unknown_type_directory .. "/*.jsonl"), {
  log_name("current"),
}), "unknown dirent types must fall back to lstat")
vim.fn.delete(unknown_type_directory, "rf")

local nonblocking_lock_stat = {
  dev = 1,
  ino = 2,
  mode = 384,
  mtime = { nsec = 0, sec = NOW_SECONDS },
  nlink = 1,
  size = 10,
  type = "file",
  uid = 1000,
}
local lock_open_flags = nil
local nonblocking_lock_fs = {
  fs_lstat = function()
    return nonblocking_lock_stat
  end,
  fs_open = function(_, flags)
    lock_open_flags = flags
    return nil, "ENOENT: injected replacement"
  end,
}
local nonblocking_lock_store = storage.new({
  directory = "/unused",
  fs = nonblocking_lock_fs,
  user_id = 1000,
})
assert(nonblocking_lock_store:_lock_owner_alive("/unused/session.lock") == false)
assert(
  lock_open_flags == vim.uv.constants.O_RDONLY + vim.uv.constants.O_NONBLOCK,
  "collector lock reads must not block on a replaced FIFO"
)

local tie_directory = vim.fn.tempname()
vim.fn.mkdir(tie_directory, "p", 448)
write_at(vim.fs.joinpath(tie_directory, log_name("alpha")), "alpha", NOW_SECONDS - DAY_SECONDS)
write_at(vim.fs.joinpath(tie_directory, log_name("bravo")), "bravo", NOW_SECONDS - DAY_SECONDS)
local tie_store = storage.new({
  directory = tie_directory,
  now_seconds = function()
    return NOW_SECONDS
  end,
  retention = {
    max_age_days = 30,
    max_sessions = 2,
  },
})
finalize(tie_store, "current")
assert(vim.deep_equal(basenames(tie_directory .. "/*.jsonl"), {
  log_name("bravo"),
  log_name("current"),
}), "filename must deterministically break equal-mtime retention ties")
vim.fn.delete(tie_directory, "rf")

local replaced_directory = vim.fn.tempname()
vim.fn.mkdir(replaced_directory, "p", 448)
local replaced_path = vim.fs.joinpath(replaced_directory, log_name("replaced"))
write_at(replaced_path, "original", NOW_SECONDS - DAY_SECONDS)
local replaced_store = storage.new({
  directory = replaced_directory,
  now_seconds = function()
    return NOW_SECONDS
  end,
  retention = {
    max_age_days = 30,
    max_sessions = 1,
  },
})
local scanned_log = replaced_store:_finalized_logs()[1]
assert(type(scanned_log.identity) == "string", "retention scans must capture artifact identity")
assert(vim.uv.fs_unlink(replaced_path))
write_at(replaced_path, "replacement", NOW_SECONDS - DAY_SECONDS)
local unlinked, unlink_error = pcall(replaced_store._unlink_log, replaced_store, scanned_log)
assert(not unlinked and tostring(unlink_error):find("changed since retention scan", 1, true))
assert(vim.fn.readfile(replaced_path)[1] == "replacement", "retention must preserve a replacement artifact")
vim.fn.delete(replaced_directory, "rf")

local final_swap_directory = vim.fn.tempname()
vim.fn.mkdir(final_swap_directory, "p", 448)
local final_swap_path = vim.fs.joinpath(final_swap_directory, log_name("final-swap"))
write_at(final_swap_path, "original", NOW_SECONDS - DAY_SECONDS)
local final_swap_store = storage.new({
  directory = final_swap_directory,
  now_seconds = function()
    return NOW_SECONDS
  end,
  unlink_child = function(descriptor, name, expected_identity, target_path)
    return filesystem.unlink_child_if_identity(
      vim.uv,
      descriptor,
      target_path,
      name,
      expected_identity,
      artifacts.identity,
      {
        quarantine_name = ".retention-final-swap",
        rename_child = function(_, source, destination)
          assert(vim.uv.fs_rename(
            vim.fs.joinpath(final_swap_directory, source),
            vim.fs.joinpath(final_swap_directory, destination)
          ))
          if source == log_name("final-swap") then
            write_at(target_path, "replacement", NOW_SECONDS - DAY_SECONDS)
          end
          return true
        end,
      }
    )
  end,
})
local final_swap_log = final_swap_store:_finalized_logs()[1]
local final_swap_ok = pcall(final_swap_store._unlink_log, final_swap_store, final_swap_log)
assert(final_swap_ok, "retention must delete the quarantined original without touching its replacement")
assert(vim.fn.readfile(final_swap_path)[1] == "replacement", "retention must preserve a final-check replacement")
vim.fn.delete(final_swap_directory, "rf")

local quarantine_directory = vim.fn.tempname()
vim.fn.mkdir(quarantine_directory, "p", 448)
local quarantine_original = log_name("retention-quarantine")
local quarantine_original_path = vim.fs.joinpath(quarantine_directory, quarantine_original)
write_at(quarantine_original_path, "private raw artifact", NOW_SECONDS - DAY_SECONDS)
local quarantine_identity = artifacts.identity(assert(vim.uv.fs_lstat(quarantine_original_path)))
local quarantine_name = artifacts.quarantine_name(quarantine_original, quarantine_identity, string.rep("c", 16))
local quarantine_path = vim.fs.joinpath(quarantine_directory, quarantine_name)
assert(vim.uv.fs_rename(quarantine_original_path, quarantine_path))
local quarantine_store = storage.new({
  directory = quarantine_directory,
  now_seconds = function()
    return NOW_SECONDS
  end,
})
quarantine_store:_prune(nil)
assert(vim.uv.fs_lstat(quarantine_path) == nil, "retention must recover an interrupted matching quarantine")
vim.fn.delete(quarantine_directory, "rf")

local legacy_quarantine_directory = vim.fn.tempname()
vim.fn.mkdir(legacy_quarantine_directory, "p", 448)
local legacy_quarantine_original = string.rep("c", 32) .. ".jsonl"
local legacy_quarantine_original_path = vim.fs.joinpath(legacy_quarantine_directory, legacy_quarantine_original)
write_at(legacy_quarantine_original_path, "legacy private raw artifact", NOW_SECONDS - DAY_SECONDS)
local legacy_quarantine_identity = artifacts.identity(assert(vim.uv.fs_lstat(legacy_quarantine_original_path)))
local legacy_quarantine_name =
  artifacts.quarantine_name(legacy_quarantine_original, legacy_quarantine_identity, string.rep("e", 16))
local legacy_quarantine_path = vim.fs.joinpath(legacy_quarantine_directory, legacy_quarantine_name)
assert(vim.uv.fs_rename(legacy_quarantine_original_path, legacy_quarantine_path))
local custom_legacy_quarantine_store = storage.new({
  directory = legacy_quarantine_directory,
  default_directory = function()
    return legacy_quarantine_directory .. "-default"
  end,
})
custom_legacy_quarantine_store:_prune(nil)
assert(vim.uv.fs_lstat(legacy_quarantine_path) ~= nil, "custom retention must not recover legacy quarantines")
local default_legacy_quarantine_store = storage.new({
  default_directory = function()
    return legacy_quarantine_directory
  end,
})
default_legacy_quarantine_store:_prune(nil)
assert(vim.uv.fs_lstat(legacy_quarantine_path) == nil, "default retention must recover legacy quarantines")
vim.fn.delete(legacy_quarantine_directory, "rf")

for _, failure in ipairs({ "unlink", "fsync" }) do
  local cleanup_directory = vim.fn.tempname()
  vim.fn.mkdir(cleanup_directory, "p", 448)
  local cleanup_path = vim.fs.joinpath(cleanup_directory, log_name("cleanup-" .. failure))
  write_at(cleanup_path, "original", NOW_SECONDS - DAY_SECONDS)
  local close_attempts = 0
  local cleanup_fs = setmetatable({}, { __index = vim.uv })
  cleanup_fs.fs_close = function(descriptor)
    close_attempts = close_attempts + 1
    return vim.uv.fs_close(descriptor)
  end
  if failure == "fsync" then
    cleanup_fs.fs_fsync = function()
      error("injected fsync failure")
    end
  end
  local cleanup_store = storage.new({
    directory = cleanup_directory,
    fs = cleanup_fs,
    unlink_child = failure == "unlink" and function()
      error("injected unlink failure")
    end or nil,
  })
  local cleanup_log = cleanup_store:_finalized_logs()[1]
  assert(not pcall(cleanup_store._unlink_log, cleanup_store, cleanup_log))
  assert(close_attempts == 1, "retention must close its directory after a throwing " .. failure)
  vim.fn.delete(cleanup_directory, "rf")
end

local retry_directory = vim.fn.tempname()
vim.fn.mkdir(retry_directory, "p", 448)
local retry_old_path = vim.fs.joinpath(retry_directory, log_name("old"))
write_at(retry_old_path, "old", NOW_SECONDS - 31 * DAY_SECONDS)
local fail_prune_once = true
local retention_errors = {}
local retry_fs = setmetatable({}, { __index = vim.uv })
local function retry_unlink_child(descriptor, name, expected_identity, target_path)
  if target_path == retry_old_path and fail_prune_once then
    fail_prune_once = false
    return nil, "injected retention failure"
  end
  return filesystem.unlink_child_if_identity(
    retry_fs,
    descriptor,
    target_path,
    name,
    expected_identity,
    artifacts.identity
  )
end
local retry_store = storage.new({
  directory = retry_directory,
  fs = retry_fs,
  now_seconds = function()
    return NOW_SECONDS
  end,
  on_retention_error = function(error_message)
    table.insert(retention_errors, error_message)
  end,
  unlink_child = retry_unlink_child,
  retention = {
    max_age_days = 30,
    max_sessions = 1,
  },
})
local retry_session = retry_store:open_session("current")
retry_session:write({
  schema.encode(schema.session_start("current")),
  schema.encode(schema.session_end("current", 1)),
})
assert(pcall(retry_session.finish, retry_session), "retention failure must not interrupt finalization")
assert(vim.uv.fs_stat(vim.fs.joinpath(retry_directory, log_name("current"))) ~= nil)
assert(vim.uv.fs_stat(vim.fs.joinpath(retry_directory, lock_name("current"))) == nil)
assert(#retention_errors == 1, "retention failure must be reported once")
assert(vim.uv.fs_stat(retry_old_path) ~= nil, "failed retention must preserve its target")
finalize(retry_store, "later")
assert(vim.deep_equal(basenames(retry_directory .. "/*.jsonl"), { log_name("later") }))
vim.fn.delete(retry_directory, "rf")

local unavailable_directory = vim.fn.tempname()
vim.fn.mkdir(unavailable_directory, "p", 448)
write_at(
  vim.fs.joinpath(unavailable_directory, log_name("expired")),
  "expired",
  NOW_SECONDS - 31 * DAY_SECONDS
)
local unavailable_notifications = {}
local original_notify = vim.notify
vim.notify = function(message, level)
  table.insert(unavailable_notifications, { level = level, message = tostring(message) })
end
local unavailable_store = storage.new({
  directory = unavailable_directory,
  now_seconds = function()
    return NOW_SECONDS
  end,
  unlink_child = function()
    return nil, "atomic descriptor-relative rename is unavailable: /private/retention-canary"
  end,
  retention = {
    max_age_days = 30,
    max_sessions = 100,
  },
})
finalize(unavailable_store, "portable-a")
finalize(unavailable_store, "portable-b")
vim.notify = original_notify
assert(#unavailable_notifications == 2, "each deferred retention pass must be reported")
for _, notification in ipairs(unavailable_notifications) do
  assert(notification.level == vim.log.levels.WARN)
  assert(notification.message == "key-insights: retention cleanup was deferred")
  assert(not notification.message:find("retention-canary", 1, true), "raw retention errors must stay private")
end
for _, session_id in ipairs({ "portable-a", "portable-b" }) do
  assert(vim.uv.fs_stat(vim.fs.joinpath(unavailable_directory, log_name(session_id))) ~= nil)
  assert(vim.uv.fs_stat(vim.fs.joinpath(unavailable_directory, lock_name(session_id))) == nil)
end
assert(
  vim.uv.fs_stat(vim.fs.joinpath(unavailable_directory, log_name("expired"))) ~= nil,
  "unsupported cleanup must preserve the retention candidate"
)
vim.fn.delete(unavailable_directory, "rf")

local scan_budget_directory = vim.fn.tempname()
vim.fn.mkdir(scan_budget_directory, "p", 448)
local scan_next_calls = 0
local scan_budget_fs = setmetatable({}, { __index = vim.uv })
scan_budget_fs.fs_scandir = function()
  return { index = 0 }
end
scan_budget_fs.fs_scandir_next = function(request)
  request.index = request.index + 1
  scan_next_calls = scan_next_calls + 1
  if request.index <= 5 then
    return "unrelated-" .. request.index, "file"
  end
  return nil
end
local scan_budget_errors = {}
local scan_budget_store = storage.new({
  directory = scan_budget_directory,
  fs = scan_budget_fs,
  max_retention_scan_entries = 3,
  on_retention_error = function(error_message)
    table.insert(scan_budget_errors, error_message)
  end,
})
finalize(scan_budget_store, "scan-budget-current")
assert(scan_next_calls == 4, "retention must stop after observing the first over-budget entry")
assert(#scan_budget_errors == 1, "scan overflow must defer cleanup without failing finalization")
assert(vim.uv.fs_lstat(vim.fs.joinpath(scan_budget_directory, log_name("scan-budget-current"))) ~= nil)
assert(vim.uv.fs_lstat(vim.fs.joinpath(scan_budget_directory, lock_name("scan-budget-current"))) == nil)
vim.fn.delete(scan_budget_directory, "rf")

local deletion_budget_directory = vim.fn.tempname()
vim.fn.mkdir(deletion_budget_directory, "p", 448)
local deletion_now = os.time()
for _, session_id in ipairs({ "delta", "bravo", "echo", "alpha", "charlie" }) do
  write_at(
    vim.fs.joinpath(deletion_budget_directory, log_name(session_id)),
    session_id,
    deletion_now - 31 * DAY_SECONDS
  )
end
local deletion_notifications = 0
local deletion_budget_store = storage.new({
  directory = deletion_budget_directory,
  max_retention_deletions_per_pass = 2,
  now_seconds = function()
    return deletion_now
  end,
  on_retention_error = function()
    deletion_notifications = deletion_notifications + 1
  end,
  retention = {
    max_age_days = 30,
    max_sessions = 100,
  },
})
finalize(deletion_budget_store, "budget-pass-a")
assert(vim.uv.fs_lstat(vim.fs.joinpath(deletion_budget_directory, log_name("alpha"))) == nil)
assert(vim.uv.fs_lstat(vim.fs.joinpath(deletion_budget_directory, log_name("bravo"))) == nil)
for _, session_id in ipairs({ "charlie", "delta", "echo" }) do
  assert(vim.uv.fs_lstat(vim.fs.joinpath(deletion_budget_directory, log_name(session_id))) ~= nil)
end
assert(deletion_notifications == 1, "remaining eligible logs must report deferred cleanup")

finalize(deletion_budget_store, "budget-pass-b")
assert(vim.uv.fs_lstat(vim.fs.joinpath(deletion_budget_directory, log_name("charlie"))) == nil)
assert(vim.uv.fs_lstat(vim.fs.joinpath(deletion_budget_directory, log_name("delta"))) == nil)
assert(vim.uv.fs_lstat(vim.fs.joinpath(deletion_budget_directory, log_name("echo"))) ~= nil)
assert(deletion_notifications == 2, "a later finalization must retry bounded cleanup")

finalize(deletion_budget_store, "budget-pass-c")
assert(vim.uv.fs_lstat(vim.fs.joinpath(deletion_budget_directory, log_name("echo"))) == nil)
assert(deletion_notifications == 2, "cleanup convergence must stop deferred warnings")
vim.fn.delete(deletion_budget_directory, "rf")

local count_budget_directory = vim.fn.tempname()
vim.fn.mkdir(count_budget_directory, "p", 448)
for index, session_id in ipairs({ "count-a", "count-b", "count-c", "count-d", "count-e" }) do
  write_at(
    vim.fs.joinpath(count_budget_directory, log_name(session_id)),
    session_id,
    deletion_now - (10 - index) * DAY_SECONDS
  )
end
local count_notifications = 0
local count_budget_store = storage.new({
  directory = count_budget_directory,
  max_retention_deletions_per_pass = 2,
  now_seconds = function()
    return deletion_now
  end,
  on_retention_error = function()
    count_notifications = count_notifications + 1
  end,
  retention = {
    max_age_days = 30,
    max_sessions = 2,
  },
})
finalize(count_budget_store, "count-current-a")
assert(vim.uv.fs_lstat(vim.fs.joinpath(count_budget_directory, log_name("count-a"))) == nil)
assert(vim.uv.fs_lstat(vim.fs.joinpath(count_budget_directory, log_name("count-b"))) == nil)
assert(count_notifications == 1, "count pruning must defer after its shared deletion budget")
finalize(count_budget_store, "count-current-b")
assert(vim.uv.fs_lstat(vim.fs.joinpath(count_budget_directory, log_name("count-c"))) == nil)
assert(vim.uv.fs_lstat(vim.fs.joinpath(count_budget_directory, log_name("count-d"))) == nil)
assert(count_notifications == 2)
finalize(count_budget_store, "count-current-c")
assert(#basenames(count_budget_directory .. "/*.jsonl") == 2, "count pruning must converge to max_sessions")
vim.fn.delete(count_budget_directory, "rf")

local shared_budget_directory = vim.fn.tempname()
vim.fn.mkdir(shared_budget_directory, "p", 448)
for _, session_id in ipairs({ "shared-log-a", "shared-log-b" }) do
  write_at(
    vim.fs.joinpath(shared_budget_directory, log_name(session_id)),
    session_id,
    NOW_SECONDS - 31 * DAY_SECONDS
  )
end
for _, session_id in ipairs({ "shared-quarantine-a", "shared-quarantine-b" }) do
  local original_name = log_name(session_id)
  local original_path = vim.fs.joinpath(shared_budget_directory, original_name)
  write_at(original_path, session_id, NOW_SECONDS - 31 * DAY_SECONDS)
  local identity = artifacts.identity(assert(vim.uv.fs_lstat(original_path)))
  local name = artifacts.quarantine_name(original_name, identity, string.rep(session_id:sub(-1), 16))
  assert(vim.uv.fs_rename(original_path, vim.fs.joinpath(shared_budget_directory, name)))
end
local shared_budget_store = storage.new({
  directory = shared_budget_directory,
  max_retention_deletions_per_pass = 2,
  now_seconds = function()
    return NOW_SECONDS
  end,
  retention = {
    max_age_days = 30,
    max_sessions = 100,
  },
})
local before_shared = #vim.fn.readdir(shared_budget_directory)
assert(not pcall(shared_budget_store._prune, shared_budget_store, nil))
local after_shared = #vim.fn.readdir(shared_budget_directory)
assert(before_shared - after_shared == 2, "quarantine recovery and log pruning must share one deletion budget")
vim.fn.delete(shared_budget_directory, "rf")

print("Lua retention contract: ok")
