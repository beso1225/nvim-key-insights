local config = require("key-insights.config")

local defaults = config.defaults()

assert(defaults.privacy.raw_keylog == false, "raw key logging must be opt-in")
assert(defaults.privacy.capture_insert_text == false, "insert text must not be captured")
assert(defaults.privacy.capture_command_text == false, "command text must not be captured")
assert(defaults.privacy.capture_search_text == false, "search text must not be captured")
assert(defaults.privacy.store_file_paths == false, "file paths must not be stored")

assert(config.is_excluded_buffer({ buftype = "terminal", filetype = "" }, defaults))
assert(config.is_excluded_buffer({ buftype = "prompt", filetype = "" }, defaults))
assert(config.is_sensitive_name("/work/.env.production", defaults))
assert(config.is_sensitive_name("credentials.json", defaults))
assert(config.is_sensitive_name("src/main.rs", defaults) == false)
assert(config.is_sensitive_buffer({ name = "scratch", filetype = "dotenv" }))
assert(config.is_sensitive_buffer({ name = "credentials.json", filetype = "json" }))
assert(config.is_sensitive_buffer({ name = "src/main.rs", filetype = "rust" }) == false)

print("Lua privacy contract: ok")
