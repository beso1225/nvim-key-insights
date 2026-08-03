local filesystem = require("key-insights.filesystem")

local function stat(size)
  return {
    dev = 1,
    ino = 2,
    mtime = { nsec = 4, sec = 3 },
    size = size,
    type = "file",
  }
end

local function fake_reader(chunks, after_read)
  local reads = 0
  local inspections = 0
  return {
    fs_close = function()
      return true
    end,
    fs_fstat = function()
      inspections = inspections + 1
      if inspections == 1 then
        return stat(3)
      end
      return after_read or stat(3)
    end,
    fs_lstat = function()
      return stat(3)
    end,
    fs_open = function(_, flags)
      assert(flags == vim.uv.constants.O_RDONLY + vim.uv.constants.O_NONBLOCK)
      return 7
    end,
    fs_read = function()
      reads = reads + 1
      local item = chunks[reads]
      if item == false then
        return nil, nil
      end
      return item or ""
    end,
  }
end

local contents = assert(filesystem.read_bounded(fake_reader({ "a", "bc", "" }), "/report", 3))
assert(contents == "abc", "bounded reads must accept short read chunks")

local oversized, oversized_error = filesystem.read_bounded(fake_reader({ "abc" }), "/report", 2)
assert(oversized == nil and oversized_error:find("size limit", 1, true))

local failed, failure_error = filesystem.read_bounded(fake_reader({ false }), "/report", 3)
assert(failed == nil and failure_error == "failed to read file")

local changed_stat = stat(3)
changed_stat.mtime = { nsec = 5, sec = 3 }
local changed, changed_error = filesystem.read_bounded(fake_reader({ "abc", "" }, changed_stat), "/report", 3)
assert(changed == nil and changed_error:find("changed while reading", 1, true))

local root = vim.fn.tempname()
local original = vim.fs.joinpath(root, "sessions")
local moved = vim.fs.joinpath(root, "moved")
local target_name = "nvim-key-insights-race.jsonl"
vim.fn.mkdir(original, "p", 448)
vim.fn.writefile({ "original" }, vim.fs.joinpath(original, target_name))
local descriptor = assert(filesystem.open_read(vim.uv, original))
assert(vim.uv.fs_rename(original, moved))
vim.fn.mkdir(original, "p", 448)
local replacement = vim.fs.joinpath(original, target_name)
vim.fn.writefile({ "replacement" }, replacement)

assert(filesystem.unlink_child(descriptor, target_name))
assert(vim.uv.fs_close(descriptor))
assert(vim.uv.fs_lstat(vim.fs.joinpath(moved, target_name)) == nil)
assert(vim.fn.readfile(replacement)[1] == "replacement", "descriptor-relative unlink must preserve a replacement directory")
local invalid, invalid_error = filesystem.unlink_child(0, "../outside")
assert(invalid == nil and invalid_error:find("invalid", 1, true))
vim.fn.delete(root, "rf")

print("Lua filesystem contract: ok")
