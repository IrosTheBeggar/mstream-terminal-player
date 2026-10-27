#!/usr/bin/env bash
# Tar the desktop flavour for Linux.
#
#   scripts/package-linux.sh <binary> <assets dir> <out.tar.gz>
#
# <assets dir> is the repo's assets/. The archive holds one directory:
#
#   mstream-player-desktop/
#     mstream-player                      the player (desktop flavour)
#     io.mstream.player.desktop           the desktop entry
#     icons/hicolor/<n>x<n>/apps/io.mstream.player.png   16 to 512 px
#     README.txt                          the per-user install
#
# The names are the app id's (src/identity.rs): the entry's Icon= and
# StartupWMClass= and the window's own app id must agree for a dock to
# show the logo on the running window.
#
# Reproducible: GNU tar with sorted names, one fixed mtime
# (SOURCE_DATE_EPOCH, else 0), owner and group 0, modes from each file's
# kind rather than the umask, and gzip -n (no name or time in its header),
# so the same inputs give the same bytes. GNU tar is `tar` on Linux and
# `gtar` (Homebrew) on a Mac.
set -eo pipefail

if [ "$#" -ne 3 ]; then
  echo "usage: $0 <binary> <assets dir> <out.tar.gz>" >&2
  exit 2
fi
binary="$1"; assets="$2"; out="$3"
app_id="io.mstream.player"
top="mstream-player-desktop"

if tar --version 2>/dev/null | grep -q 'GNU tar'; then
  gnutar=tar
elif command -v gtar >/dev/null; then
  gnutar=gtar
else
  echo "error: needs GNU tar (on a Mac: brew install gnu-tar)" >&2
  exit 1
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
root="$work/$top"
mkdir -p "$root"

install -m 755 "$binary" "$root/mstream-player"
install -m 644 "$assets/linux/$app_id.desktop" "$root/$app_id.desktop"
count=0
for size in 16 32 48 64 128 256 512; do
  dir="icons/hicolor/${size}x${size}/apps"
  mkdir -p "$root/$dir"
  install -m 644 "$assets/icons/hicolor/${size}x${size}/apps/$app_id.png" "$root/$dir/$app_id.png"
  count=$((count + 1))
done

cat > "$root/README.txt" <<'README'
mStream Player for Linux (desktop flavour: opens in its own window)

Run it in place:   ./mstream-player

Install it for your user, so it shows in the app menu with its icon:

  mkdir -p ~/.local/bin ~/.local/share/applications ~/.local/share/icons
  cp mstream-player ~/.local/bin/
  cp io.mstream.player.desktop ~/.local/share/applications/
  cp -r icons/hicolor ~/.local/share/icons/
  update-desktop-database ~/.local/share/applications
  gtk-update-icon-cache ~/.local/share/icons/hicolor   # if your desktop uses it

The menu entry runs `mstream-player`, so ~/.local/bin must be on your PATH
(it is on most distributions once the directory exists; log out and in).

On X11 the window needs libxkbcommon-x11 (Debian/Ubuntu: libxkbcommon-x11-0,
Fedora: libxkbcommon-x11); audio needs ALSA (libasound2 / alsa-lib).
From a terminal, `mstream-player --help` lists the other ways to run it.
README
chmod 644 "$root/README.txt"

mtime="${SOURCE_DATE_EPOCH:-0}"
mkdir -p "$(dirname "$out")"
"$gnutar" --sort=name --mtime="@$mtime" --owner=0 --group=0 --numeric-owner \
  --mode='u=rwX,go=rX' --format=gnu -C "$work" -cf - "$top" | gzip -9n > "$out"

echo "$out ($count icons)"
