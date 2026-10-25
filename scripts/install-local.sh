#!/usr/bin/env bash
set -euo pipefail
umask 077
# Generated release scripts embed authenticated artifact URLs and hashes here.
platform="$(uname -s)-$(uname -m)"
agent_url=''
agent_sha=''
case "$platform" in
# @ARTIFACTS@
  *) echo "No native local release for $platform" >&2; exit 1 ;;
esac
command -v curl >/dev/null || { echo 'curl is required' >&2; exit 1; }
task_stage=$(mktemp -d)
trap 'rm -rf "$task_stage"' EXIT
curl --proto '=https' --tlsv1.2 -fSL "$agent_url" -o "$task_stage/shellmcp-agent"
test -s "$task_stage/shellmcp-agent" || { echo 'Empty agent download' >&2; exit 1; }
if command -v sha256sum >/dev/null; then
  actual=$(sha256sum "$task_stage/shellmcp-agent")
else
  actual=$(shasum -a 256 "$task_stage/shellmcp-agent")
fi
actual=${actual%% *}
test "$actual" = "$agent_sha" || { echo 'Agent SHA256 mismatch' >&2; exit 1; }
chmod 700 "$task_stage/shellmcp-agent"
"$task_stage/shellmcp-agent" local install "$@"
