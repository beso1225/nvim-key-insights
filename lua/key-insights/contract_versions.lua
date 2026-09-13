local M = {
  event_log = 4,
  analysis_summary = 6,
  keymap_snapshot = 1,
  codex_payload = 4,
  codex_suggestions = 3,
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
    [4] = true,
    [5] = true,
    [6] = true,
  },
}

return M
