local keymap_snapshot = require("key-insights.keymap_snapshot")
local key_tokens = require("key-insights.key_tokens")
local mapping_resolver = require("key-insights.mapping_resolver")

local function entry(lhs, fields)
  return vim.tbl_extend("force", {
    lhs = "display-value-must-not-win",
    lhsraw = lhs,
  }, fields or {})
end

local function dependencies(overrides)
  local current_buffer = 7
  local globals = { n = {} }
  local locals = { n = {} }
  local live = {}
  local deps = {
    current_buffer = function()
      return current_buffer
    end,
    get_buffer_keymaps = function(buffer, mode)
      assert(buffer == current_buffer)
      return locals[mode] or {}
    end,
    get_global_keymaps = function(mode)
      return globals[mode] or {}
    end,
    keytrans = function(value)
      return value
    end,
    maparg = function(lhs, mode)
      return live[mode .. "\0" .. lhs] or {}
    end,
    sha256 = vim.fn.sha256,
  }
  return vim.tbl_extend("force", deps, overrides or {}), {
    globals = globals,
    locals = locals,
    live = live,
    set_buffer = function(buffer)
      current_buffer = buffer
    end,
  }
end

local function mapping_id(mode, scope, lhs)
  return assert(keymap_snapshot.mapping_id(mode, scope, assert(key_tokens.tokenize(lhs)), {
    sha256 = vim.fn.sha256,
  }))
end

local stable_deps, stable_state = dependencies()
stable_state.globals.n = { entry("zq") }
stable_state.live["n\0zq"] = entry("zq", { buffer = 0 })
local stable = mapping_resolver.new(stable_deps)
assert(stable:prime(7) == true)
assert(vim.deep_equal(stable:resolve("normal", { "z", "q" }), {
  mapping_id = mapping_id("normal", "global", "zq"),
  mode = "normal",
  scope = "global",
}))
stable:reset()
assert(stable:resolve("normal", { "z", "q" }) == nil, "reset must invalidate the baseline")

local shadow_deps, shadow_state = dependencies()
shadow_state.globals.n = { entry("zq") }
shadow_state.locals.n = { entry("zq", { buffer = 1 }) }
shadow_state.live["n\0zq"] = entry("zq", { buffer = 1 })
local shadow = mapping_resolver.new(shadow_deps)
assert(shadow:prime(7) == true)
local shadowed = assert(shadow:resolve("normal", { "z", "q" }))
assert(shadowed.scope == "buffer")
assert(shadowed.mapping_id == mapping_id("normal", "buffer", "zq"))

local prefix_deps, prefix_state = dependencies()
prefix_state.globals.n = { entry("z"), entry("zq") }
prefix_state.live["n\0z"] = entry("z", { buffer = 0 })
prefix_state.live["n\0zq"] = entry("zq", { buffer = 0 })
local prefix = mapping_resolver.new(prefix_deps)
assert(prefix:prime(7) == true)
assert(prefix:resolve("normal", { "z" }) == nil)
assert(prefix:resolve("normal", { "z", "q" }) == nil)

local mutation_deps, mutation_state = dependencies()
mutation_state.globals.n = { entry("zq") }
mutation_state.live["n\0zq"] = entry("zr", { buffer = 0 })
local mutation = mapping_resolver.new(mutation_deps)
assert(mutation:prime(7) == true)
assert(mutation:resolve("normal", { "z", "q" }) == nil, "changed mappings must fail closed")

local buffer_deps, buffer_state = dependencies()
buffer_state.globals.n = { entry("zq") }
buffer_state.live["n\0zq"] = entry("zq", { buffer = 0 })
local changed_buffer = mapping_resolver.new(buffer_deps)
assert(changed_buffer:prime(7) == true)
buffer_state.set_buffer(8)
assert(changed_buffer:resolve("normal", { "z", "q" }) == nil, "buffer changes must invalidate a baseline")

local failing_prime = mapping_resolver.new(select(1, dependencies({
  get_global_keymaps = function()
    error("api-secret")
  end,
})))
local prime_ok, prime_result = pcall(failing_prime.prime, failing_prime, 7)
assert(prime_ok and prime_result ~= true, "prime API errors must fail closed")

local failing_live_deps, failing_live_state = dependencies({
  maparg = function()
    error("live-api-secret")
  end,
})
failing_live_state.globals.n = { entry("zq") }
local failing_live = mapping_resolver.new(failing_live_deps)
assert(failing_live:prime(7) == true)
local resolve_ok, resolve_result = pcall(failing_live.resolve, failing_live, "normal", { "z", "q" })
assert(resolve_ok and resolve_result == nil, "live API errors must fail closed")

local previous_buffer = vim.api.nvim_get_current_buf()
local real_buffer = vim.api.nvim_create_buf(false, true)
vim.api.nvim_set_current_buf(real_buffer)
local real_ok, real_error = xpcall(function()
  vim.keymap.set("n", "<F19>", "j", { noremap = true })
  vim.keymap.set("n", "<F20>", "j", { noremap = true })
  vim.keymap.set("n", "<F20>", "k", { buffer = real_buffer, noremap = true })
  local real = assert(mapping_resolver.new())
  assert(real:prime(real_buffer) == true)
  local global = assert(real:resolve("normal", { "<F19>" }))
  local buffer_local = assert(real:resolve("normal", { "<F20>" }))
  assert(global.scope == "global")
  assert(buffer_local.scope == "buffer")
end, debug.traceback)
pcall(vim.keymap.del, "n", "<F19>")
pcall(vim.keymap.del, "n", "<F20>")
pcall(vim.keymap.del, "n", "<F20>", { buffer = real_buffer })
if vim.api.nvim_buf_is_valid(previous_buffer) then
  vim.api.nvim_set_current_buf(previous_buffer)
end
pcall(vim.api.nvim_buf_delete, real_buffer, { force = true })
assert(real_ok, real_error)

print("Lua mapping resolver: ok")
