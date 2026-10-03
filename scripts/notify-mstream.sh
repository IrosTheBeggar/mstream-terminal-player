#!/bin/sh
# Tell mStream a release is out so its pin-bump PR opens itself: one
# repository_dispatch (event_type player-release, client_payload.tag) to
# IrosTheBeggar/mStream, whose update-mstream-player-manifest workflow
# consumes it — regenerates bin/mstream-player/manifest.json, re-downloads
# and sha256-verifies every binary against this release's manifest.json,
# and opens or updates a bump PR that a human merges there. The tag it
# currently pins makes it print "pins already current — nothing to do" (an
# older tag would open a downgrade PR).
#
# Reads TAG and TOKEN from the environment. Called by release.yml's notify
# job (TAG = github.ref_name, stable tags only) and by notify-mstream.yml's
# hand crank (TAG = the operator's input).
#
# TOKEN is MSTREAM_REPO_TOKEN: a fine-grained PAT — Repository access:
# IrosTheBeggar/mStream only; Permissions: Contents: Read and write.
#
# Failure modes. A TAG that is empty or carries a character outside
# [A-Za-z0-9._-] is an error before any network call: the tag goes into a
# JSON body, and mStream's workflow validates it with the same pattern. A
# missing TOKEN warns and exits 0, because a secret that was never set must
# not fail a published release. Anything GitHub refuses exits non-zero with
# GitHub's own body in the log (an expired token answers 401, one that
# cannot see the repo 404, one that sees it without Contents: write 403) —
# that is what --fail-with-body is for; plain -f would hide it.
set -eu

TAG="${TAG:-}"
TOKEN="${TOKEN:-}"

case "$TAG" in
  '' | *[!A-Za-z0-9._-]*)
    printf '%s\n' "::error::TAG must match ^[A-Za-z0-9._-]+\$ (got '$TAG') — mStream was not notified"
    exit 1
    ;;
esac

if [ -z "$TOKEN" ]; then
  echo "::warning::MSTREAM_REPO_TOKEN is not set — mStream was not notified; run its update-mstream-player-manifest workflow by hand for $TAG"
  exit 0
fi

curl -sS --fail-with-body -X POST \
  -H "Authorization: Bearer $TOKEN" \
  -H "Accept: application/vnd.github+json" \
  -H "X-GitHub-Api-Version: 2022-11-28" \
  https://api.github.com/repos/IrosTheBeggar/mStream/dispatches \
  -d "{\"event_type\":\"player-release\",\"client_payload\":{\"tag\":\"$TAG\"}}"
echo "notified mStream: $TAG"
