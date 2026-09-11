local M = {
  event_log = 3,
  analysis_summary = 5,
  keymap_snapshot = 1,
  codex_payload = 2,
  codex_suggestions = 1,
  ergonomics = 2,
  histogram = 1,
  operation_token_set = 1,
  count_prefix_token_set = 1,
  directional_motion_token_set = 1,
  candidate_kind = 1,
  report_summary_versions = {
    [1] = true,
    [2] = true,
    [3] = true,
    [5] = true,
  },
}

return M
