local attribution = require("key-insights.mapping_attribution")

local function only_candidate(overrides)
  return {
    vim.tbl_extend("force", {
      exact = true,
      lhs = { "z", "q" },
      mode = "normal",
      scope = "global",
      stable = true,
    }, overrides or {}),
  }
end

assert(attribution.classify_callback("j", "zq") == "typed_different")
assert(attribution.classify_callback("q", "q") == "typed_same")
assert(attribution.classify_callback("j", "") == "untyped")
assert(attribution.classify_callback("", "zq") == "typed_without_output")
assert(attribution.classify_callback(nil, "zq") == "unsupported")
assert(attribution.classify_callback("j", string.rep("q", 4097)) == "unsupported")
assert(attribution.classify_callback(string.rep("j", 4097), "q") == "unsupported")

local mapped_secret = table.concat({ "<Cmd>edit ", "/private/credential", "\0<CR>" })
local secret_evidence = attribution.classify_callback(mapped_secret, "zq")
assert(secret_evidence == "typed_different")
assert(string.find(vim.inspect(secret_evidence), mapped_secret, 1, true) == nil)

local sanitized = attribution.confirm(only_candidate({
  callback = function() end,
  desc = "private description",
  rhs = ":edit /private/path<CR>",
}), "typed_different")
assert(vim.deep_equal(sanitized, {
  mode = "normal",
  scope = "global",
}))
assert(string.find(vim.inspect(sanitized), "private", 1, true) == nil)

local hostile_lhs = table.concat({ "/private/credential", "\0", "not-a-token" })
local hostile_result = attribution.confirm(only_candidate({ lhs = { hostile_lhs } }), "typed_same")
assert(vim.deep_equal(hostile_result, { mode = "normal", scope = "global" }))
assert(string.find(vim.inspect(hostile_result), hostile_lhs, 1, true) == nil)

assert(attribution.confirm({}, "typed_different") == nil)
assert(attribution.confirm({ only_candidate()[1], only_candidate()[1] }, "typed_different") == nil)
assert(attribution.confirm({ [2] = only_candidate()[1] }, "typed_different") == nil)
assert(attribution.confirm(only_candidate({ exact = false }), "typed_different") == nil)
assert(attribution.confirm(only_candidate({ stable = false }), "typed_different") == nil)
assert(attribution.confirm(only_candidate(), "untyped") == nil)
assert(attribution.confirm(only_candidate({ mode = "insert" }), "typed_same") == nil)
assert(attribution.confirm(only_candidate({ scope = "window" }), "typed_same") == nil)

local namespace = vim.api.nvim_create_namespace("key-insights.mapping-attribution-test")

local function capture(spec)
  local previous_buffer = vim.api.nvim_get_current_buf()
  local buffer = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_set_current_buf(buffer)
  vim.api.nvim_buf_set_lines(buffer, 0, -1, false, { "alpha beta", "gamma delta" })

  local traces = {}
  local callback_result = nil
  local listener = function(mapped, typed)
    callback_result = attribution.classify_callback(mapped, typed)
    table.insert(traces, {
      evidence = callback_result,
      mode = vim.api.nvim_get_mode().mode,
      typed = vim.fn.keytrans(typed),
    })
    return nil
  end

  local map_options = vim.deepcopy(spec.options or {})
  if map_options.noremap == nil and map_options.remap == nil then
    map_options.noremap = true
  end
  local mapping_created = false
  local final_cursor = nil
  local ok, error_message = xpcall(function()
    if spec.setup ~= nil then
      spec.setup(buffer)
    end
    vim.keymap.set(spec.map_mode, spec.lhs, spec.rhs, map_options)
    mapping_created = true
    vim.on_key(listener, namespace)
    vim.api.nvim_feedkeys(vim.api.nvim_replace_termcodes(spec.input, true, false, true), "xt", false)
    final_cursor = vim.api.nvim_win_get_cursor(0)
  end, debug.traceback)

  vim.on_key(nil, namespace)
  local delete_options = map_options.buffer and { buffer = buffer } or nil
  local delete_ok, delete_error = true, nil
  if mapping_created then
    delete_ok, delete_error = pcall(vim.keymap.del, spec.map_mode, spec.lhs, delete_options)
  end
  local cleanup_ok, cleanup_error = true, nil
  if spec.cleanup ~= nil then
    cleanup_ok, cleanup_error = pcall(spec.cleanup, buffer)
  end
  if vim.api.nvim_buf_is_valid(previous_buffer) then
    vim.api.nvim_set_current_buf(previous_buffer)
  end
  pcall(vim.api.nvim_buf_delete, buffer, { force = true })
  assert(ok, error_message)
  assert(delete_ok, delete_error)
  assert(cleanup_ok, cleanup_error)
  local trace_count = #traces
  assert(listener("x", "x") == nil, "the attribution listener must not consume input")
  assert(callback_result == "typed_same")
  assert(#traces == trace_count + 1)
  table.remove(traces)
  return traces, final_cursor
end

local normal, normal_cursor = capture({
  input = "zq",
  lhs = "zq",
  map_mode = "n",
  rhs = "j",
})
assert(#normal == 1)
assert(vim.deep_equal(normal[1], {
  evidence = "typed_different",
  mode = "n",
  typed = "zq",
}))
assert(normal_cursor[1] == 2, "the listener must not consume the mapped motion")

local ambiguous_short = capture({
  cleanup = function()
    vim.keymap.del("n", "zxy")
  end,
  input = "zxk",
  lhs = "zx",
  map_mode = "n",
  rhs = "j",
  setup = function()
    vim.keymap.set("n", "zxy", "k", { noremap = true })
  end,
})
assert(#ambiguous_short == 2)
assert(ambiguous_short[1].typed == "zx" and ambiguous_short[1].evidence == "typed_different")
assert(ambiguous_short[2].typed == "k" and ambiguous_short[2].evidence == "typed_same")

local ambiguous_long = capture({
  cleanup = function()
    vim.keymap.del("n", "zx")
  end,
  input = "zxy",
  lhs = "zxy",
  map_mode = "n",
  rhs = "k",
  setup = function()
    vim.keymap.set("n", "zx", "j", { noremap = true })
  end,
})
assert(#ambiguous_long == 1)
assert(ambiguous_long[1].typed == "zxy" and ambiguous_long[1].evidence == "typed_different")

local recursive = capture({
  cleanup = function()
    vim.keymap.del("n", "<Plug>(KeyInsightsRecursive)")
  end,
  input = "zr",
  lhs = "zr",
  map_mode = "n",
  options = { remap = true },
  rhs = "<Plug>(KeyInsightsRecursive)",
  setup = function()
    vim.keymap.set("n", "<Plug>(KeyInsightsRecursive)", "j", { noremap = true })
  end,
})
assert(#recursive == 1 and recursive[1].typed == "zr")
assert(recursive[1].evidence == "typed_different")

local buffer_local = capture({
  input = "zb",
  lhs = "zb",
  map_mode = "n",
  options = { buffer = true },
  rhs = "j",
})
assert(#buffer_local == 1 and buffer_local[1].typed == "zb")
assert(buffer_local[1].evidence == "typed_different")

local visual = capture({
  input = "vZ<Esc>",
  lhs = "Z",
  map_mode = "x",
  rhs = "l",
})
assert(visual[2].typed == "Z" and visual[2].mode == "v")
assert(visual[2].evidence == "typed_different")

local operator_pending = capture({
  input = "dZ",
  lhs = "Z",
  map_mode = "o",
  rhs = "w",
})
assert(operator_pending[2].typed == "Z" and vim.startswith(operator_pending[2].mode, "no"))
assert(operator_pending[2].evidence == "typed_different")

local identity = capture({
  input = "zi",
  lhs = "zi",
  map_mode = "n",
  rhs = "zi",
})
assert(identity[1].typed == "zi" and identity[1].evidence == "typed_different")
assert(identity[2].typed == "" and identity[2].evidence == "untyped")

local callback_secret = table.concat({ "mapping", "callback", "secret" }, "-")
local callback_executed = false
local callback = capture({
  input = "zc",
  lhs = "zc",
  map_mode = "n",
  rhs = function()
    callback_executed = callback_secret ~= ""
  end,
})
assert(callback_executed == true)
assert(callback[1].typed == "zc" and callback[1].evidence == "typed_different")
assert(string.find(vim.json.encode(callback), callback_secret, 1, true) == nil)

local nop = capture({
  input = "zn",
  lhs = "zn",
  map_mode = "n",
  rhs = "<Nop>",
})
assert(nop[1].typed == "zn" and nop[1].evidence == "typed_different")

print("Lua mapping attribution contract: ok")
