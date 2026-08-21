#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
marketplace="$repo_root/.agents/plugins/marketplace.json"
plugin="$repo_root/plugins/nvim-key-insights"
skill="$plugin/skills/analyze-neovim-usage"

python3 - "$repo_root" <<'PY'
import json
import pathlib
import re
import sys

root = pathlib.Path(sys.argv[1])
marketplace_path = root / ".agents/plugins/marketplace.json"
plugin_path = root / "plugins/nvim-key-insights"
manifest_path = plugin_path / ".codex-plugin/plugin.json"
skill_path = plugin_path / "skills/analyze-neovim-usage"

required_files = {
    ".codex-plugin/plugin.json",
    "skills/analyze-neovim-usage/SKILL.md",
    "skills/analyze-neovim-usage/agents/openai.yaml",
    "skills/analyze-neovim-usage/references/payload.schema.json",
    "skills/analyze-neovim-usage/references/suggestions.schema.json",
}

if not marketplace_path.is_file():
    raise SystemExit("missing repository Codex marketplace")
if not plugin_path.is_dir():
    raise SystemExit("missing nvim-key-insights Codex plugin")

marketplace = json.loads(marketplace_path.read_text())
if set(marketplace) != {"name", "interface", "plugins"}:
    raise SystemExit("marketplace root fields drifted")
if marketplace["name"] != "nvim-key-insights":
    raise SystemExit("unexpected marketplace name")
if marketplace["interface"] != {"displayName": "nvim-key-insights"}:
    raise SystemExit("unexpected marketplace interface")
expected_entry = {
    "name": "nvim-key-insights",
    "source": {"source": "local", "path": "./plugins/nvim-key-insights"},
    "policy": {"installation": "AVAILABLE", "authentication": "ON_INSTALL"},
    "category": "Productivity",
}
if marketplace["plugins"] != [expected_entry]:
    raise SystemExit("marketplace must expose exactly the local plugin")

manifest = json.loads(manifest_path.read_text())
allowed_manifest_fields = {
    "name",
    "version",
    "description",
    "author",
    "homepage",
    "repository",
    "keywords",
    "skills",
    "interface",
}
if set(manifest) != allowed_manifest_fields:
    raise SystemExit("plugin manifest fields drifted")
if manifest["name"] != "nvim-key-insights" or manifest["skills"] != "./skills/":
    raise SystemExit("plugin identity or skill path is invalid")
if manifest["author"] != {
    "name": "beso1225",
    "url": "https://github.com/beso1225",
}:
    raise SystemExit("plugin author metadata drifted")
if manifest["homepage"] != "https://github.com/beso1225/nvim-key-insights":
    raise SystemExit("plugin homepage drifted")
if manifest["repository"] != "https://github.com/beso1225/nvim-key-insights":
    raise SystemExit("plugin repository drifted")
if manifest["keywords"] != ["neovim", "keymaps", "ergonomics", "privacy"]:
    raise SystemExit("plugin keywords drifted")

interface = manifest["interface"]
required_interface = {
    "displayName",
    "shortDescription",
    "longDescription",
    "developerName",
    "category",
    "capabilities",
    "defaultPrompt",
}
if set(interface) != required_interface:
    raise SystemExit("plugin interface fields drifted")
if interface["displayName"] != "nvim-key-insights":
    raise SystemExit("unexpected plugin display name")
if interface["developerName"] != "beso1225":
    raise SystemExit("unexpected plugin developer")
if interface["category"] != "Productivity" or interface["capabilities"] != []:
    raise SystemExit("plugin must remain an inert skill-only package")

cargo = (root / "crates/key-insights-cli/Cargo.toml").read_text()
flake = (root / "flake.nix").read_text()
cargo_version = re.search(r'^version = "([^"]+)"$', cargo, re.MULTILINE)
flake_version = re.search(r'^\s*version = "([^"]+)";$', flake, re.MULTILINE)
if not cargo_version or not flake_version:
    raise SystemExit("could not read repository versions")
versions = {manifest["version"], cargo_version.group(1), flake_version.group(1)}
if len(versions) != 1:
    raise SystemExit("plugin, Cargo, and flake versions must match")

actual_files = set()
for path in plugin_path.rglob("*"):
    if path.is_symlink():
        raise SystemExit(f"plugin tree must not contain symlinks: {path}")
    if path.is_file():
        actual_files.add(path.relative_to(plugin_path).as_posix())
        if path.stat().st_mode & 0o111:
            raise SystemExit(f"plugin data file must not be executable: {path}")
if actual_files != required_files:
    raise SystemExit(
        "plugin tree is not allowlisted: "
        f"missing={sorted(required_files - actual_files)} "
        f"unexpected={sorted(actual_files - required_files)}"
    )

skill_text = (skill_path / "SKILL.md").read_text()
if "[TODO:" in skill_text:
    raise SystemExit("skill contains scaffold placeholders")
frontmatter = re.match(r"^---\nname: ([^\n]+)\ndescription: ([^\n]+)\n---\n", skill_text)
if not frontmatter or frontmatter.group(1) != "analyze-neovim-usage":
    raise SystemExit("skill frontmatter is invalid")
if len(frontmatter.group(2).strip()) < 40:
    raise SystemExit("skill description is not informative")

openai_yaml = (skill_path / "agents/openai.yaml").read_text()
required_metadata = (
    'display_name: "Analyze Neovim Usage"',
    'short_description: "Review sanitized Neovim usage evidence"',
    'default_prompt: "Use $analyze-neovim-usage to evaluate my sanitized Neovim usage summary."',
)
if not all(item in openai_yaml for item in required_metadata):
    raise SystemExit("skill UI metadata drifted")
PY

cmp "$repo_root/codex/suggestions.schema.json" \
  "$skill/references/suggestions.schema.json"
cmp "$repo_root/codex/payload.schema.json" \
  "$skill/references/payload.schema.json"

echo "Codex plugin contract: ok"
