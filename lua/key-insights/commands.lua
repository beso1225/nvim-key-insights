local M = {}

local function notify_status(api)
  local status = api.status()
  local message = "key-insights: " .. status.state
  if status.session_id ~= nil then
    message = message .. " (session " .. status.session_id .. ")"
  end
  if status.report_running then
    message = message .. " (report running)"
  end
  vim.notify(message, vim.log.levels.INFO)
end

function M.register(api)
  vim.api.nvim_create_user_command("KeyInsightsStart", function()
    api.start()
  end, { desc = "Start or resume privacy-safe key insights collection", force = true })

  vim.api.nvim_create_user_command("KeyInsightsPause", function()
    api.pause()
  end, { desc = "Pause key insights collection", force = true })

  vim.api.nvim_create_user_command("KeyInsightsStop", function()
    api.stop()
  end, { desc = "Stop collection and close the current session", force = true })

  vim.api.nvim_create_user_command("KeyInsightsStatus", function()
    notify_status(api)
  end, { desc = "Show key insights collector status", force = true })

  vim.api.nvim_create_user_command("KeyInsightsReport", function()
    api.report()
  end, { desc = "Generate and open the local key insights report", force = true })

  vim.api.nvim_create_user_command("KeyInsightsOpenReport", function()
    api.open_report()
  end, { desc = "Open the existing local key insights report", force = true })

  vim.api.nvim_create_user_command("KeyInsightsPurge", function(command)
    api.purge(command.bang)
  end, { bang = true, desc = "Purge collector-owned session artifacts", force = true })
end

return M
