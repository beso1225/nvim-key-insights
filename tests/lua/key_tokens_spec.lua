local key_tokens = require("key-insights.key_tokens")

local function assert_failure(value, limits, expected)
  local tokens, error_code = key_tokens.tokenize(value, limits)
  assert(tokens == nil)
  assert(error_code == expected, vim.inspect(error_code))
end

assert(vim.deep_equal(assert(key_tokens.tokenize("")), {}))
assert(vim.deep_equal(assert(key_tokens.tokenize("abc")), { "a", "b", "c" }))
assert(vim.deep_equal(assert(key_tokens.tokenize("日本語")), { "日", "本", "語" }))
assert(vim.deep_equal(assert(key_tokens.tokenize("<C-X>a<Space>")), { "<C-X>", "a", "<Space>" }))

-- A missing closing bracket was historically collected as literal characters.
assert(vim.deep_equal(assert(key_tokens.tokenize("<C-X")), { "<", "C", "-", "X" }))

-- Angle notation longer than the collector's legacy 256-byte grouping bound is
-- also treated literally. Keep this stable when the tokenizer is shared.
local oversized_notation = "<" .. string.rep("x", 255) .. ">"
local oversized_tokens = assert(key_tokens.tokenize(oversized_notation))
assert(#oversized_tokens == 257)
assert(oversized_tokens[1] == "<" and oversized_tokens[#oversized_tokens] == ">")

assert_failure(nil, nil, "key_tokens:invalid_input")
assert_failure({}, nil, "key_tokens:invalid_input")
assert_failure("a", { max_tokens = -1 }, "key_tokens:invalid_limits")
assert_failure("a", { max_token_bytes = 1.5 }, "key_tokens:invalid_limits")
assert_failure("", { max_tokens = -1 }, "key_tokens:invalid_limits")

assert(vim.deep_equal(assert(key_tokens.tokenize("", { max_tokens = 0, max_token_bytes = 0 })), {}))
assert_failure("abc", { max_tokens = 2 }, "key_tokens:limit_exceeded")
assert_failure("<C-X>", { max_token_bytes = 4 }, "key_tokens:limit_exceeded")
assert(vim.deep_equal(assert(key_tokens.tokenize("日", { max_tokens = 1, max_token_bytes = 3 })), { "日" }))
assert_failure("日", { max_token_bytes = 2 }, "key_tokens:limit_exceeded")

local secret = "private-secret-token"
local _, limit_error = key_tokens.tokenize("<" .. secret .. ">", { max_token_bytes = 4 })
assert(limit_error == "key_tokens:limit_exceeded")
assert(string.find(limit_error, secret, 1, true) == nil)
