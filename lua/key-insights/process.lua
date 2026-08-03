local M = {}

function M.run(argv, callback)
  return vim.system(argv, { text = true }, vim.schedule_wrap(callback))
end

return M
