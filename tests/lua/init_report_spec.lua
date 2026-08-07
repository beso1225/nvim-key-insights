local module_names = {
  "key-insights",
  "key-insights.collector",
  "key-insights.commands",
  "key-insights.config",
  "key-insights.purge",
  "key-insights.report",
  "key-insights.storage",
}
local saved = {}
for _, name in ipairs(module_names) do
  saved[name] = package.loaded[name]
end

local resolved_options = {
  collection = { max_sequence_keys = 64 },
  report = { analyzer = "key-insights", directory = "/state/reports" },
  storage = {},
}
local report_config = nil
local report_starts = 0

package.loaded["key-insights"] = nil
package.loaded["key-insights.collector"] = { new = function() error("collector must remain lazy") end }
package.loaded["key-insights.commands"] = { register = function() end }
package.loaded["key-insights.config"] = {
  defaults = function()
    return resolved_options
  end,
}
package.loaded["key-insights.purge"] = { new = function() error("purge must remain lazy") end }
package.loaded["key-insights.report"] = {
  default_directory = function()
    return "/default/reports"
  end,
  new = function(config)
    report_config = config
    return {
      start = function()
        report_starts = report_starts + 1
        return true
      end,
      status = function()
        return { running = false }
      end,
    }
  end,
}
package.loaded["key-insights.storage"] = {
  new = function()
    return { directory = "/state/sessions" }
  end,
}

local isolated = require("key-insights")
assert(isolated.report() == true)
assert(isolated.report() == true)
assert(report_starts == 2 and report_config ~= nil)
assert(report_config.collector_options == resolved_options, "report snapshot collection must use resolved collector options")
assert(report_config.output_directory == "/state/reports")
assert(report_config.session_directory == "/state/sessions")

for _, name in ipairs(module_names) do
  package.loaded[name] = saved[name]
end

print("Lua report initialization wiring contract: ok")
