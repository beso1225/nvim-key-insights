local M = {}

function M.run(argv, callback, stdin)
  return vim.system(argv, { text = true, stdin = stdin }, vim.schedule_wrap(callback))
end

return M
