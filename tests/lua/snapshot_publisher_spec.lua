local publisher = require("key-insights.snapshot_publisher")

local OWNER_READ_WRITE = 384 -- 0600
local OWNER_DIRECTORY = 448 -- 0700

local function read(path)
  local stat = assert(vim.uv.fs_stat(path))
  local descriptor = assert(vim.uv.fs_open(path, "r", 0))
  local contents = assert(vim.uv.fs_read(descriptor, stat.size, 0))
  assert(vim.uv.fs_close(descriptor))
  return contents
end

local function make_directory()
  local directory = vim.fn.tempname()
  assert(vim.fn.mkdir(directory, "p", OWNER_DIRECTORY) >= 0)
  assert(vim.uv.fs_chmod(directory, OWNER_DIRECTORY))
  return directory
end

local encoded = '{"snapshot_version":1,"mappings":[]}\n'
local model = { snapshot_version = 1, mappings = {} }

local directory = make_directory()
local instance = publisher.new({ output_directory = directory }, {
  collect_snapshot = function()
    return model
  end,
  encode_snapshot = function(value)
    assert(value == model)
    return encoded
  end,
  slot_seed = function() return 0 end,
})

local published_path, _, published_digest = assert(instance:publish())
assert(published_path == vim.fs.joinpath(directory, "keymap-snapshot-00.json"))
assert(string.match(published_digest, "^sha256:[0-9a-f]+$") ~= nil)
assert(read(published_path) == encoded)
local published_stat = assert(vim.uv.fs_lstat(published_path))
assert(published_stat.type == "file" and published_stat.nlink == 1)
assert(published_stat.mode % 4096 == OWNER_READ_WRITE, "published snapshots must be private")

local second = publisher.new({ output_directory = directory }, {
  collect_snapshot = function()
    return model
  end,
  encode_snapshot = function()
    return encoded
  end,
  slot_seed = function() return 1 end,
})
local second_path = assert(second:publish())
assert(second_path ~= published_path, "each publication must return an immutable invocation-specific path")
assert(read(published_path) == encoded, "later publication must not mutate a prior invocation's snapshot")

local reused_path, reused_identity = assert(second:publish())
assert(reused_path == second_path, "identical content must reuse its immutable snapshot")
assert(reused_identity ~= nil, "reused snapshots must retain filesystem identity pinning")

local failure_slot = 2
local function assert_failure_preserves_prior(name, dependencies, expected_error)
  local slot = failure_slot
  failure_slot = failure_slot + 1
  local old_contents = read(published_path)
  local failed = publisher.new({ output_directory = directory }, vim.tbl_extend("force", {
    collect_snapshot = function()
      return model
    end,
    encode_snapshot = function()
      return encoded
    end,
    slot_seed = function() return slot end,
  }, dependencies or {}))
  local path, error_code = failed:publish()
  assert(path == nil and error_code == expected_error)
  assert(read(published_path) == old_contents, "failed publication must preserve prior snapshots")
  assert(read(published_path) == old_contents, name .. " must preserve prior snapshots")
end

assert_failure_preserves_prior("collect-failure", {
  collect_snapshot = function()
    return nil, "keymap_snapshot:api_failed secret-buffer-name"
  end,
}, "snapshot_publisher:collection_failed")

assert_failure_preserves_prior("oversized", {
  encode_snapshot = function()
    return nil, "keymap_snapshot:limit_exceeded secret-mapping"
  end,
}, "snapshot_publisher:encoding_failed")

local write_fs = setmetatable({
  fs_write = function()
    return nil, "injected write failure secret-payload"
  end,
}, { __index = vim.uv })
assert_failure_preserves_prior("write-failure", { fs = write_fs }, "snapshot_publisher:write_failed")

local replaced_fs = setmetatable({}, { __index = vim.uv })
local lstat_calls = 0
replaced_fs.fs_lstat = function(path)
  local stat, stat_error = vim.uv.fs_lstat(path)
  if path == directory and stat ~= nil then
    lstat_calls = lstat_calls + 1
    stat = vim.deepcopy(stat)
    if lstat_calls > 1 then
      stat.ino = stat.ino + 1
    end
  end
  return stat, stat_error
end
assert_failure_preserves_prior("replaced-directory", { fs = replaced_fs }, "snapshot_publisher:directory_changed")

local symlink_path = vim.fs.joinpath(directory, "keymap-snapshot-0a.json")
local symlink_target = vim.fs.joinpath(directory, "outside.json")
assert(vim.fn.writefile({ "outside" }, symlink_target) == 0)
assert(vim.uv.fs_symlink(symlink_target, symlink_path))
local prior_before_symlink = read(published_path)
local symlink_instance = publisher.new({ output_directory = directory }, {
  collect_snapshot = function()
    return model
  end,
  encode_snapshot = function()
    return encoded
  end,
  slot_seed = function() return 10 end,
})
local symlink_result = assert(symlink_instance:publish())
assert(symlink_result ~= symlink_path, "an occupied slot must be skipped")
assert(assert(vim.uv.fs_lstat(symlink_path)).type == "link", "publication must not replace a symlink")
assert(read(symlink_target) == "outside\n")
assert(read(published_path) == prior_before_symlink)

local retention_directory = make_directory()
for index = 1, 16 do
  local path = vim.fs.joinpath(retention_directory, string.format("keymap-snapshot-%02x.json", index - 1))
  assert(vim.fn.writefile({ encoded }, path) == 0)
  assert(vim.uv.fs_chmod(path, OWNER_READ_WRITE))
end
local retention_instance = publisher.new({ output_directory = retention_directory }, {
  collect_snapshot = function() return model end,
  encode_snapshot = function() return encoded .. "new" end,
  slot_seed = function() return 0 end,
})
local retention_path, retention_error = retention_instance:publish()
assert(retention_path == nil and retention_error == "snapshot_publisher:retention_limit")
vim.fn.delete(retention_directory, "rf")

vim.fn.delete(directory, "rf")

print("Lua sanitized snapshot publication contract: ok")
