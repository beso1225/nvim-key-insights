local artifacts = require("key-insights.artifacts")
local schema = require("key-insights.schema")

local session_directory = assert(os.getenv("KEY_INSIGHTS_SESSION_DIR"))
local report_directory = assert(os.getenv("KEY_INSIGHTS_REPORT_DIR"))
local trace_path = assert(os.getenv("KEY_INSIGHTS_TRACE_PATH"))
local analyzer = assert(os.getenv("KEY_INSIGHTS_BIN"))
local now = os.time()

assert(vim.fn.mkdir(session_directory, "p", 448) >= 0)
local function write_private(name, contents, modified_at)
  local path = vim.fs.joinpath(session_directory, name)
  vim.fn.writefile(contents, path)
  assert(vim.uv.fs_chmod(path, 384))
  assert(vim.uv.fs_utime(path, modified_at, modified_at))
end

local function finalized(session_id, modified_at)
  write_private(artifacts.name(session_id, ".jsonl"), {
    string.sub(schema.encode(schema.session_start(session_id)), 1, -2),
    string.sub(schema.encode(schema.session_end(session_id, 1)), 1, -2),
  }, modified_at)
end

finalized("expired", now - 3 * 24 * 60 * 60)
finalized("old-a", now - 30)
finalized("old-b", now - 20)
finalized("live", now - 10)
write_private(artifacts.name("live", ".lock"), {
  vim.json.encode({ pid = vim.fn.getpid(), version = 1 }),
}, now - 10)
write_private(artifacts.name("incomplete", ".jsonl.part"), { "incomplete" }, now - 4 * 24 * 60 * 60)
write_private("unrelated.txt", { "unrelated" }, now - 4 * 24 * 60 * 60)
local incomplete_path = vim.fs.joinpath(session_directory, artifacts.name("incomplete", ".jsonl.part"))
local unrelated_path = vim.fs.joinpath(session_directory, "unrelated.txt")
local preserved_before = {
  incomplete = artifacts.identity(assert(vim.uv.fs_lstat(incomplete_path))),
  unrelated = artifacts.identity(assert(vim.uv.fs_lstat(unrelated_path))),
}

local api = require("key-insights").setup({
  storage = {
    directory = session_directory,
    retention = { max_age_days = 1, max_sessions = 2 },
  },
  report = { analyzer = analyzer, directory = report_directory },
})
vim.cmd.KeyInsightsStart()
local current_session = assert(api.status().session_id)
vim.cmd.KeyInsightsStop()

local names = {}
local request = assert(vim.uv.fs_scandir(session_directory))
while true do
  local name = vim.uv.fs_scandir_next(request)
  if name == nil then
    break
  end
  table.insert(names, name)
end
table.sort(names)
vim.fn.writefile({ vim.json.encode({
  current_session = current_session,
  names = names,
  preserved_before = preserved_before,
  preserved_after = {
    incomplete = artifacts.identity(assert(vim.uv.fs_lstat(incomplete_path))),
    unrelated = artifacts.identity(assert(vim.uv.fs_lstat(unrelated_path))),
  },
}) }, trace_path)
