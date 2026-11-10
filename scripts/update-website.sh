#!/bin/sh
# Stamp a release into mstream.io: write {version, tag} into the `player`
# block of IrosTheBeggar/mstream-website's src/data/release.json — the
# site's single source of truth for its download links — commit, and push
# to the site's master, whose push deploys the site (its bunny.yml).
#
# The site links the desktop packages by exact file name under the stamped
# tag (src/data/downloads.ts there, desktopPackages), so every one of them
# must be on the release before it is stamped: a half-published release, or
# one whose desktop family was held back (release.yml, build-desktop), would
# put a row of 404s on the download band. PACKAGES below is that list; keep
# the two in step.
#
# Reads from the environment:
#   TAG       the release to stamp, a stable vX.Y.Z
#   SITE_DIR  a checkout of the site's master, made with a key that may push
#             (release.yml's website job and update-website.yml check it out
#             with WEBSITE_KEY)
#   GH_TOKEN  for `gh release view` on this repo
#   REPO      this repo, default $GITHUB_REPOSITORY
#   ALLOW_DOWNGRADE=1  stamp a tag older than the one the site shows (a
#             rollback; otherwise refused, so a patch release for an older
#             line never takes the site back)
#   DRY_RUN=1 stamp and show the diff, commit and push nothing
#
# Failure modes. A TAG that is not a stable vX.Y.Z is an error before any
# network call; pre-releases never reach the site. A release that is a
# draft or marked pre-release, or is missing any package, fails with the
# names. The site already at TAG is a green no-op — a re-run, or the hand
# crank's proof that the key works. The JSON is written the way the mStream
# repo's update-website.yaml writes its `server` block (JSON.stringify, two
# spaces, trailing newline), so neither stamp reformats the other's. A push
# that loses a race with another stamp is retried on the new tip, the stamp
# re-applied rather than merged; a push refused with no race behind it is a
# key problem, and fails at once saying so.
set -eu

TAG="${TAG:-}"
SITE_DIR="${SITE_DIR:-}"
REPO="${REPO:-${GITHUB_REPOSITORY:-IrosTheBeggar/mstream-terminal-player}}"

PACKAGES="mstream-player-desktop-win32-x64.zip
mstream-player-desktop-darwin-arm64.app.zip
mstream-player-desktop-darwin-x64.app.zip
mstream-player-desktop-linux-x64.tar.gz"

if ! printf '%s\n' "$TAG" | grep -Eq '^v[0-9]+\.[0-9]+\.[0-9]+$'; then
  printf '%s\n' "::error::TAG must be a stable vX.Y.Z (got '$TAG') — mstream.io was not updated"
  exit 1
fi
VERSION="${TAG#v}"
if [ -z "$SITE_DIR" ] || [ ! -f "$SITE_DIR/src/data/release.json" ]; then
  echo "::error::SITE_DIR must be a checkout of mstream-website (no src/data/release.json in '$SITE_DIR')"
  exit 1
fi

# One call: the first line is "<isDraft> <isPrerelease>", then an asset name a line.
release=$(gh release view "$TAG" --repo "$REPO" --json isDraft,isPrerelease,assets \
  -q '"\(.isDraft) \(.isPrerelease)", .assets[].name')
state=$(printf '%s\n' "$release" | head -n 1)
have=$(printf '%s\n' "$release" | tail -n +2)
if [ "$state" != "false false" ]; then
  echo "::error::$TAG is a draft or a pre-release (isDraft isPrerelease: $state) — mstream.io was not updated"
  exit 1
fi
missing=0
for f in $PACKAGES; do
  if ! printf '%s\n' "$have" | grep -qxF "$f"; then
    echo "::error::$TAG has no $f — the site's download band would 404; mstream.io was not updated"
    missing=1
  fi
done
[ "$missing" = 0 ]

cd "$SITE_DIR"

# Reads the stamped version, or writes the stamp; the site's own `player`
# block is the only thing this touches.
stamp() {
  node -e '
    const fs = require("fs");
    const p = "src/data/release.json";
    const j = JSON.parse(fs.readFileSync(p, "utf8"));
    if (process.argv[1] === "read") { console.log(j.player?.version ?? ""); process.exit(0); }
    j.player = { version: process.env.VERSION, tag: process.env.TAG };
    fs.writeFileSync(p, JSON.stringify(j, null, 2) + "\n");
  ' "$1"
}
export VERSION TAG

current=$(stamp read)
if [ -n "$current" ] && [ "$current" != "$VERSION" ] &&
   [ "$(printf '%s\n%s\n' "$current" "$VERSION" | sort -V | tail -1)" = "$current" ]; then
  if [ "${ALLOW_DOWNGRADE:-}" != 1 ]; then
    echo "::error::the site shows $current, newer than $TAG — not rolling it back (ALLOW_DOWNGRADE=1 does)"
    exit 1
  fi
  echo "rolling the site back from $current to $VERSION"
fi

if [ "${DRY_RUN:-}" = 1 ]; then
  stamp write
  git --no-pager diff -- src/data/release.json
  git checkout -q -- src/data/release.json
  echo "dry run: nothing committed or pushed"
  exit 0
fi

git config user.name "github-actions[bot]"
git config user.email "github-actions[bot]@users.noreply.github.com"
attempt=1
while :; do
  base=$(git rev-parse HEAD)
  stamp write
  git add src/data/release.json
  if git diff --cached --quiet; then
    echo "site already at $TAG — nothing to stamp"
    exit 0
  fi
  git commit -q -m "action: player release $TAG"
  if git push -q origin HEAD:master; then
    echo "stamped $TAG — the site's Bunny deploy takes it from here"
    exit 0
  fi
  git fetch -q origin master
  if [ "$(git rev-parse FETCH_HEAD)" = "$base" ]; then
    echo "::error::the site's master refused the push and has not moved — is WEBSITE_KEY a deploy key with write access on mstream-website?"
    exit 1
  fi
  if [ "$attempt" -ge 3 ]; then
    echo "::error::the site's master kept moving; gave up after $attempt tries — run update-website.yml for $TAG"
    exit 1
  fi
  echo "the site's master moved under the push; stamping again on its new tip"
  attempt=$((attempt + 1))
  git reset -q --hard FETCH_HEAD
done
