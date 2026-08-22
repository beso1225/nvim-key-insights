local M = {
  event_log = 1,
  analysis_summary = 3,
  keymap_snapshot = 1,
  codex_payload = 1,
  codex_suggestions = 1,
  ergonomics = 1,
  histogram = 1,
  operation_token_set = 1,
  count_prefix_token_set = 1,
  directional_motion_token_set = 1,
  candidate_kind = 1,
  report_summary_versions = {
    [1] = true,
    [2] = true,
    [3] = true,
  },
}

return M
