local config = require("key-insights.config")
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

local retry_directory = vim.fn.tempname()
vim.fn.mkdir(retry_directory, "p", 448)
local retry_old_path = vim.fs.joinpath(retry_directory, log_name("old"))
write_at(retry_old_path, "old", NOW_SECONDS - DAY_SECONDS)
local fail_prune_once = true
local retry_fs = setmetatable({}, { __index = vim.uv })
retry_fs.fs_unlink = function(path)
  if path == retry_old_path and fail_prune_once then
    fail_prune_once = false
    return nil, "injected retention failure"
  end
  return vim.uv.fs_unlink(path)
end
local retry_store = storage.new({
  directory = retry_directory,
  fs = retry_fs,
  now_seconds = function()
    return NOW_SECONDS
  end,
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
assert(not pcall(retry_session.finish, retry_session), "retention failure must be reported")
assert(vim.uv.fs_stat(vim.fs.joinpath(retry_directory, log_name("current"))) ~= nil)
assert(vim.uv.fs_stat(vim.fs.joinpath(retry_directory, lock_name("current"))) ~= nil)
retry_session:finish()
assert(vim.deep_equal(basenames(retry_directory .. "/*.jsonl"), { log_name("current") }))
assert(vim.uv.fs_stat(vim.fs.joinpath(retry_directory, lock_name("current"))) == nil)
vim.fn.delete(retry_directory, "rf")

print("Lua retention contract: ok")
