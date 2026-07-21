if vim.g.loaded_key_insights == 1 then
  return
end
vim.g.loaded_key_insights = 1

require("key-insights").register_commands()
