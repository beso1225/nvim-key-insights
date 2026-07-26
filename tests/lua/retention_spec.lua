local config = require("key-insights.config")
local schema = require("key-insights.schema")
local storage = require("key-insights.storage")

local DAY_SECONDS = 24 * 60 * 60
local NOW_SECONDS = 2000000000

local function write_at(path, contents, modified_at)
  vim.fn.writefile({ contents }, path)
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
  { max_age_days = 0 },
  { max_age_days = 1.5 },
}) do
  assert(pcall(function()
    config.resolve({ storage = { retention = invalid } })
  end) == false, "invalid retention values must be rejected")
end

local directory = vim.fn.tempname()
vim.fn.mkdir(directory, "p", 448)
write_at(vim.fs.joinpath(directory, "expired.jsonl"), "expired", NOW_SECONDS - 31 * DAY_SECONDS)
write_at(vim.fs.joinpath(directory, "count-old.jsonl"), "count-old", NOW_SECONDS - 10 * DAY_SECONDS)
write_at(vim.fs.joinpath(directory, "recent-a.jsonl"), "recent-a", NOW_SECONDS - 2 * DAY_SECONDS)
write_at(vim.fs.joinpath(directory, "recent-b.jsonl"), "recent-b", NOW_SECONDS - DAY_SECONDS)
write_at(vim.fs.joinpath(directory, "boundary.jsonl"), "boundary", NOW_SECONDS - 30 * DAY_SECONDS)
write_at(vim.fs.joinpath(directory, "crashed.jsonl.part"), "partial", NOW_SECONDS - 90 * DAY_SECONDS)
write_at(vim.fs.joinpath(directory, "crashed.lock"), "lock", NOW_SECONDS - 90 * DAY_SECONDS)
write_at(vim.fs.joinpath(directory, "notes.jsonl.backup"), "unrelated", NOW_SECONDS - 90 * DAY_SECONDS)

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
  "count-old.jsonl",
  "current.jsonl",
  "recent-a.jsonl",
  "recent-b.jsonl",
}), "retention must remove expired logs before pruning the oldest excess sessions: " .. vim.inspect(retained_names))
for _, protected_name in ipairs({ "crashed.jsonl.part", "crashed.lock", "notes.jsonl.backup" }) do
  assert(vim.uv.fs_stat(vim.fs.joinpath(directory, protected_name)) ~= nil, protected_name .. " must not be pruned")
end
vim.fn.delete(directory, "rf")

local boundary_directory = vim.fn.tempname()
vim.fn.mkdir(boundary_directory, "p", 448)
write_at(vim.fs.joinpath(boundary_directory, "boundary.jsonl"), "boundary", NOW_SECONDS - 30 * DAY_SECONDS)
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
  "boundary.jsonl",
  "current.jsonl",
}), "a log exactly at the age boundary must be retained")
vim.fn.delete(boundary_directory, "rf")

local locked_directory = vim.fn.tempname()
vim.fn.mkdir(locked_directory, "p", 448)
write_at(vim.fs.joinpath(locked_directory, "locked.jsonl"), "locked", NOW_SECONDS - 90 * DAY_SECONDS)
write_at(vim.fs.joinpath(locked_directory, "locked.lock"), "lock", NOW_SECONDS - 90 * DAY_SECONDS)
local locked_store = storage.new({
  directory = locked_directory,
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
  "current.jsonl",
  "locked.jsonl",
}), "a finalized log with an active lock must remain protected")
vim.uv.fs_unlink(vim.fs.joinpath(locked_directory, "locked.lock"))
finalize(locked_store, "later")
assert(vim.deep_equal(basenames(locked_directory .. "/*.jsonl"), { "later.jsonl" }))
vim.fn.delete(locked_directory, "rf")

local tie_directory = vim.fn.tempname()
vim.fn.mkdir(tie_directory, "p", 448)
write_at(vim.fs.joinpath(tie_directory, "alpha.jsonl"), "alpha", NOW_SECONDS - DAY_SECONDS)
write_at(vim.fs.joinpath(tie_directory, "bravo.jsonl"), "bravo", NOW_SECONDS - DAY_SECONDS)
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
  "bravo.jsonl",
  "current.jsonl",
}), "filename must deterministically break equal-mtime retention ties")
vim.fn.delete(tie_directory, "rf")

local retry_directory = vim.fn.tempname()
vim.fn.mkdir(retry_directory, "p", 448)
local retry_old_path = vim.fs.joinpath(retry_directory, "old.jsonl")
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
assert(vim.uv.fs_stat(vim.fs.joinpath(retry_directory, "current.jsonl")) ~= nil)
assert(vim.uv.fs_stat(vim.fs.joinpath(retry_directory, "current.lock")) ~= nil)
retry_session:finish()
assert(vim.deep_equal(basenames(retry_directory .. "/*.jsonl"), { "current.jsonl" }))
assert(vim.uv.fs_stat(vim.fs.joinpath(retry_directory, "current.lock")) == nil)
vim.fn.delete(retry_directory, "rf")

print("Lua retention contract: ok")
