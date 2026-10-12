#!/usr/bin/env bash
# Waits for the GitHub Actions run of a commit and prints one line:
#   RESULT <run id> <conclusion> <url>
# and exits 0 only if the run succeeded. Finds the run by commit (and
# optionally by branch or tag), retrying until GitHub has registered it,
# so a just-pushed commit never yields an empty run id.
#
#   scripts/ci-wait.sh <owner/repo> <sha> [branch-or-tag]
set -euo pipefail
repo=$1 sha=$2 ref=${3:-}
filter='.[0].databaseId // empty'
[ -n "$ref" ] && filter="[.[] | select(.headBranch == \"$ref\")][0].databaseId // empty"
id=""
for _ in $(seq 1 60); do
	id=$(gh run list -R "$repo" --commit "$sha" --json databaseId,headBranch -q "$filter")
	[ -n "$id" ] && break
	sleep 5
done
if [ -z "$id" ]; then
	echo "RESULT none no-run-found-for-$sha"
	exit 2
fi
gh run watch "$id" -R "$repo" --interval 30 >/dev/null 2>&1 || true
read -r conclusion url < <(gh run view "$id" -R "$repo" --json conclusion,url -q '"\(.conclusion) \(.url)"')
echo "RESULT $id $conclusion $url"
[ "$conclusion" = success ]
