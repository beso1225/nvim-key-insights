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

print("Lua filesystem contract: ok")
