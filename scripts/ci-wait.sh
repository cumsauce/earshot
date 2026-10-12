#!/usr/bin/env bash
# Waits for the GitHub Actions run of a commit and prints one line:
#   RESULT <run id> <conclusion> <url>
# and exits 0 only if the run succeeded. Finds the run by commit (and
# optionally by branch or tag), retrying until GitHub has registered it,
# so a just-pushed commit never yields an empty run id.
#
#   scripts/ci-wait.sh <owner/repo> <sha-or-tag> [branch-or-tag]
#
# A tag or branch name in place of the sha is resolved to its commit here, so
# callers never write `^{commit}` in a shell that globs it (zsh does).
set -euo pipefail
repo=$1 sha=$2 ref=${3:-}
if ! [[ $sha =~ ^[0-9a-f]{40}$ ]]; then
	resolved=$(git rev-parse "$sha^{commit}" 2>/dev/null) || { echo "RESULT none cannot-resolve-$sha"; exit 2; }
	[ -z "$ref" ] && ref=$sha
	sha=$resolved
fi
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
# Poll rather than `gh run watch`, which can hang without a terminal.
for _ in $(seq 1 120); do
	status=$(gh run view "$id" -R "$repo" --json status -q .status 2>/dev/null) || status=error
	[ "$status" = completed ] && break
	[ "$status" = error ] && { echo "RESULT $id gh-error"; exit 3; }
	sleep 30
done
read -r conclusion url < <(gh run view "$id" -R "$repo" --json conclusion,url -q '"\(.conclusion) \(.url)"')
echo "RESULT $id $conclusion $url"
[ "$conclusion" = success ]
