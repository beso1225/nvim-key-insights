local config = require("key-insights.config")
local schema = require("key-insights.schema")

local defaults = config.defaults()

assert(defaults.privacy.raw_keylog == false, "raw key logging must be opt-in")
assert(defaults.privacy.capture_insert_text == false, "insert text must not be captured")
assert(defaults.privacy.capture_command_text == false, "command text must not be captured")
assert(defaults.privacy.capture_search_text == false, "search text must not be captured")
assert(defaults.privacy.store_file_paths == false, "file paths must not be stored")
assert(defaults.collection.max_sequence_keys == 64)
assert(defaults.collection.sequence_timeout_ms == 1000)
assert(defaults.storage.retention.max_age_days == 30)
assert(defaults.storage.retention.max_sessions == 100)
assert(defaults.report.analyzer == "key-insights")
assert(defaults.report.directory == nil)
assert(pcall(config.resolve, { collection = { max_sequence_keys = 0 } }) == false)
assert(pcall(config.resolve, { collection = { max_sequence_keys = 65537 } }) == false)
assert(pcall(config.resolve, { collection = { max_sequence_keys = math.huge } }) == false)
assert(pcall(config.resolve, { collection = { sequence_timeout_ms = -1 } }) == false)
assert(pcall(config.resolve, { collection = { sequence_timeout_ms = math.huge } }) == false)
assert(pcall(config.resolve, { report = { analyzer = "" } }) == false)
assert(pcall(config.resolve, { report = { directory = "" } }) == false)

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
local long_key = string.rep("k", 1024)
assert(
  schema.key_sequence("session-one", 20, "normal", { long_key }, 1).keys[1] == long_key,
  "schema-v1 key token compatibility must be preserved"
)

local mapping = schema.mapping_use("session-one", 25, "normal", "mapping-42", { "<leader>", "f" })
assert(mapping.mapping_id == "mapping-42")
assert(mapping.typed_keys[1] == "<leader>")
assert(mapping.mapped_keys == nil, "mapping RHS must not be stored")
local long_mapping_id = string.rep("m", 1024)
assert(
  schema.mapping_use("session-one", 25, "normal", long_mapping_id, { "g" }).mapping_id == long_mapping_id,
  "schema-v1 mapping ID compatibility must be preserved"
)

local encoded = schema.encode(text_run)
assert(string.sub(encoded, -1) == "\n", "JSONL records must end with one newline")
local decoded = vim.json.decode(encoded)
assert(decoded.key_count == 4)
assert(decoded.text == nil)

print("Lua privacy contract: ok")
