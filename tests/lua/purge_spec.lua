local artifacts = require("key-insights.artifacts")
local purge = require("key-insights.purge")

local PREFIX = "nvim-key-insights-"

assert(artifacts.is_private_file({ type = "file", nlink = 1, mode = 2432, uid = 1000 }, 1000) == false)
assert(artifacts.is_private_directory({ type = "directory", mode = 960, uid = 1000 }, 1000) == false)
assert(artifacts.is_private_file({ type = "file", nlink = 1, uid = 1000 }, 1000) == false)
assert(artifacts.is_private_file({ type = "file", nlink = 1, mode = 384, uid = 1000 }, nil) == false)
assert(artifacts.current_user_id({}) == nil, "platforms without getuid must not fail during setup")

local function path(directory, name)
  return vim.fs.joinpath(directory, name)
end

local function write_private(file_path, contents)
  vim.fn.writefile({ contents }, file_path)
  assert(vim.uv.fs_chmod(file_path, 384))
end

local function names(items)
  local result = vim.tbl_map(function(item)
    return item.name
  end, items)
  table.sort(result)
  return result
end

local function exists(file_path)
  return vim.uv.fs_lstat(file_path) ~= nil
end

local directory = vim.fn.tempname()
vim.fn.mkdir(directory, "p", 448)

local removable = {
  PREFIX .. "complete.jsonl",
  PREFIX .. "orphan.jsonl.part",
  PREFIX .. "stale.jsonl.part",
  PREFIX .. "stale.lock",
}
for _, name in ipairs(removable) do
  local contents = name:match("%.lock$") and vim.json.encode({ pid = 101, version = 1 }) or "event"
  write_private(path(directory, name), contents)
end

local protected = {
  PREFIX .. "active.jsonl.part",
  PREFIX .. "active.lock",
  PREFIX .. "live.jsonl.part",
  PREFIX .. "live.lock",
  PREFIX .. "invalid.jsonl.part",
  PREFIX .. "invalid.lock",
}
for _, name in ipairs(protected) do
  local contents = "event"
  if name == PREFIX .. "active.lock" then
    contents = vim.json.encode({ pid = 202, version = 1 })
  elseif name == PREFIX .. "live.lock" then
    contents = vim.json.encode({ pid = 303, version = 1 })
  elseif name == PREFIX .. "invalid.lock" then
    contents = "not-json"
  end
  write_private(path(directory, name), contents)
end

local unrelated = path(directory, "notes.txt")
write_private(unrelated, "keep")

local wrong_mode = path(directory, PREFIX .. "wide.jsonl")
write_private(wrong_mode, "keep")
assert(vim.uv.fs_chmod(wrong_mode, 420))

local hard_link = path(directory, PREFIX .. "linked.jsonl")
local hard_link_peer = path(directory, "linked-peer")
write_private(hard_link, "keep")
assert(vim.uv.fs_link(hard_link, hard_link_peer))

local symlink = path(directory, PREFIX .. "symlink.jsonl")
assert(vim.uv.fs_symlink(unrelated, symlink))

local matching_directory = path(directory, PREFIX .. "directory.jsonl")
vim.fn.mkdir(matching_directory, "p", 448)

local confirmation = nil
local instance = purge.new({
  active_session_id = function()
    return "active"
  end,
  directory = directory,
}, {
  confirm = function(message)
    confirmation = message
    return false
  end,
  is_process_alive = function(pid)
    return pid == 303
  end,
  notify = function() end,
})

local preview = instance:preview()
assert(vim.deep_equal(names(preview.targets), removable), "preview must contain only provably owned stale artifacts")
assert(#preview.protected == #protected, "active, live, and invalid-lock sessions must be protected")
assert(preview.skipped == 6, "unrelated, unsafe-mode, linked, symlink, and directory entries must be skipped")

local cancelled = instance:run(false)
assert(cancelled.cancelled == true and cancelled.removed == 0, "cancelling purge must not mutate storage")
assert(confirmation:find(PREFIX .. "complete.jsonl", 1, true), "confirmation must list bounded target names")
for _, name in ipairs(removable) do
  assert(exists(path(directory, name)), "cancelled purge removed " .. name)
end

local result = instance:run(true)
assert(result.cancelled == false)
assert(result.removed == #removable and result.failed == 0)
for _, name in ipairs(removable) do
  assert(not exists(path(directory, name)), "forced purge did not remove " .. name)
end
for _, name in ipairs(protected) do
  assert(exists(path(directory, name)), "purge removed protected artifact " .. name)
end
for _, file_path in ipairs({ unrelated, wrong_mode, hard_link, hard_link_peer, symlink, matching_directory }) do
  assert(exists(file_path), "purge removed an unowned or unsafe entry: " .. file_path)
end
vim.fn.delete(directory, "rf")

local changed_leaf_directory = vim.fn.tempname()
vim.fn.mkdir(changed_leaf_directory, "p", 448)
local changed_leaf_path = path(changed_leaf_directory, PREFIX .. "changed.jsonl")
write_private(changed_leaf_path, "before")
local changed_leaf_purge = purge.new({ directory = changed_leaf_directory }, { notify = function() end })
local changed_leaf_preview = changed_leaf_purge:preview()
assert(vim.uv.fs_unlink(changed_leaf_path))
write_private(changed_leaf_path, "replacement")
local changed_leaf_result = changed_leaf_purge:apply(changed_leaf_preview)
assert(changed_leaf_result.removed == 0 and changed_leaf_result.failed == 1)
assert(exists(changed_leaf_path), "a replacement leaf must never be unlinked")
vim.fn.delete(changed_leaf_directory, "rf")

local late_lock_directory = vim.fn.tempname()
vim.fn.mkdir(late_lock_directory, "p", 448)
local late_target = path(late_lock_directory, PREFIX .. "claimed.jsonl.part")
write_private(late_target, "partial")
local late_lock_purge = purge.new({ directory = late_lock_directory }, {
  is_process_alive = function(pid)
    return pid == 404
  end,
  notify = function() end,
})
local late_lock_preview = late_lock_purge:preview()
write_private(path(late_lock_directory, PREFIX .. "claimed.lock"), vim.json.encode({ pid = 404, version = 1 }))
local late_lock_result = late_lock_purge:apply(late_lock_preview)
assert(late_lock_result.removed == 0 and late_lock_result.protected == 2)
assert(exists(late_target), "a session claimed after preview must remain protected")
vim.fn.delete(late_lock_directory, "rf")

local changed_directory = vim.fn.tempname()
vim.fn.mkdir(changed_directory, "p", 448)
local moved_directory = changed_directory .. "-moved"
local directory_target_name = PREFIX .. "directory-race.jsonl"
write_private(path(changed_directory, directory_target_name), "original")
local changed_directory_purge = purge.new({ directory = changed_directory }, { notify = function() end })
local changed_directory_preview = changed_directory_purge:preview()
assert(vim.uv.fs_rename(changed_directory, moved_directory))
vim.fn.mkdir(changed_directory, "p", 448)
write_private(path(changed_directory, directory_target_name), "replacement")
assert(pcall(changed_directory_purge.apply, changed_directory_purge, changed_directory_preview) == false)
assert(exists(path(moved_directory, directory_target_name)), "the original directory must remain untouched after replacement")
assert(exists(path(changed_directory, directory_target_name)), "the replacement directory must remain untouched")
vim.fn.delete(changed_directory, "rf")
vim.fn.delete(moved_directory, "rf")

local bounded_directory = vim.fn.tempname()
vim.fn.mkdir(bounded_directory, "p", 448)
write_private(path(bounded_directory, PREFIX .. "one.jsonl"), "one")
write_private(path(bounded_directory, PREFIX .. "two.jsonl"), "two")
local scan_bounded = purge.new({ directory = bounded_directory }, {
  max_entries = 1,
  notify = function() end,
})
assert(pcall(scan_bounded.preview, scan_bounded) == false, "an oversized directory scan must fail closed")
local target_bounded = purge.new({ directory = bounded_directory }, {
  max_targets = 1,
  notify = function() end,
})
assert(pcall(target_bounded.preview, target_bounded) == false, "an oversized target set must fail closed")
vim.fn.delete(bounded_directory, "rf")

local missing_directory = vim.fn.tempname()
local missing_result = purge.new({ directory = missing_directory }, { notify = function() end }):run(true)
assert(missing_result.removed == 0 and missing_result.failed == 0)

local legacy_directory = vim.fn.tempname()
vim.fn.mkdir(legacy_directory, "p", 448)
local legacy_name = string.rep("a", 32) .. ".jsonl"
write_private(path(legacy_directory, legacy_name), "legacy")
local legacy_excluded = purge.new({ directory = legacy_directory }, { notify = function() end }):preview()
assert(#legacy_excluded.targets == 0 and legacy_excluded.skipped == 1)
local legacy_included = purge.new({ directory = legacy_directory, include_legacy = true }, {
  notify = function() end,
}):run(true)
assert(legacy_included.removed == 1, "legacy artifacts must require the storage compatibility opt-in")
vim.fn.delete(legacy_directory, "rf")

local failure_directory = vim.fn.tempname()
vim.fn.mkdir(failure_directory, "p", 448)
local failure_name = PREFIX .. "failure.jsonl"
local success_name = PREFIX .. "success.jsonl"
write_private(path(failure_directory, failure_name), "failure")
write_private(path(failure_directory, success_name), "success")
local failure_fs = setmetatable({}, { __index = vim.uv })
failure_fs.fs_unlink = function(file_path)
  if file_path == path(failure_directory, failure_name) then
    return nil, "EACCES: injected purge failure"
  end
  return vim.uv.fs_unlink(file_path)
end
local partial_failure = purge.new({ directory = failure_directory }, {
  fs = failure_fs,
  notify = function() end,
}):run(true)
assert(partial_failure.removed == 1 and partial_failure.failed == 1)
assert(exists(path(failure_directory, failure_name)), "a failed unlink must be reported and left intact")
assert(not exists(path(failure_directory, success_name)), "one unlink failure must not stop bounded cleanup")
vim.fn.delete(failure_directory, "rf")
