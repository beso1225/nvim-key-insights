local config = require("key-insights.config")
local schema = require("key-insights.schema")

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

local start = schema.session_start("session-one", "project-one")
assert(start.schema_version == 1)
assert(start.event_type == "session_start")
assert(start.session_id == "session-one")
assert(start.elapsed_ms == 0)
assert(start.project_id == "project-one")

local text_run = schema.text_run("session-one", 15, 4, 10)
assert(text_run.event_type == "text_run")
assert(text_run.key_count == 4)
assert(text_run.duration_ms == 10)
assert(text_run.text == nil, "text_run must not have a text field")

local sequence = schema.key_sequence("session-one", 20, "normal", { "d", "d" }, 5)
assert(sequence.mode == "normal")
assert(sequence.keys[1] == "d")
assert(sequence.mapped == nil, "mapping RHS must not enter key_sequence events")

local ok = pcall(schema.key_sequence, "session-one", 20, "insert", { "s" }, 1)
assert(ok == false, "Insert-mode sequences must be rejected")

local mapping = schema.mapping_use("session-one", 25, "normal", "mapping-42", { "<leader>", "f" })
assert(mapping.mapping_id == "mapping-42")
assert(mapping.typed_keys[1] == "<leader>")
assert(mapping.mapped_keys == nil, "mapping RHS must not be stored")

local encoded = schema.encode(text_run)
assert(string.sub(encoded, -1) == "\n", "JSONL records must end with one newline")
local decoded = vim.json.decode(encoded)
assert(decoded.key_count == 4)
assert(decoded.text == nil)

print("Lua privacy contract: ok")
