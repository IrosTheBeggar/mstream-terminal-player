#!/usr/bin/env python3
"""Zip the desktop flavour for Windows: the portable package.

    scripts/package-windows.py <player.exe> <stub.exe> <icon.ico> <out.zip>

<player.exe> is the desktop-flavour binary and <stub.exe> the launcher
stub (src/bin/launch.rs) from the same build. The zip holds, at its root:

    mStream Player.exe   the stub, the one to double-click
    mstream-player.exe   the player, under the name the stub looks for
    mstream-player.ico   the logo, for a shortcut someone makes by hand
    README.txt

Python's zipfile rather than `zip`, so the archive is byte-for-byte the
same from the same inputs on any host (release.yml runs it on Linux, the
local check on a Mac): fixed member order, a fixed timestamp and fixed
permission bits, none of them read from the filesystem. The timestamp is
SOURCE_DATE_EPOCH when set (release.yml sets the commit's time), else the
DOS epoch.
"""

import os
import sys
import time
import zipfile

# The build is not code-signed: Windows signing needs a certificate the
# project does not have yet, so SmartScreen warns on first run. Said here
# because the zip is all a downloader sees.
README = (
    'Double-click "mStream Player.exe" to open mStream Player.\r\n'
    "The mstream-player.exe beside it is the player itself, which also works from a terminal (mstream-player.exe --help).\r\n"
    'This build is not code-signed, so Windows SmartScreen may warn on first run: choose "More info", then "Run anyway".\r\n'
)


def stamp():
    """The members' timestamp: SOURCE_DATE_EPOCH, clamped to the zip
    format's 1980 floor, else that floor."""
    floor = (1980, 1, 1, 0, 0, 0)
    epoch = os.environ.get("SOURCE_DATE_EPOCH")
    if not epoch:
        return floor
    t = time.gmtime(int(epoch))[:6]
    return max(t, floor)


def member(name, mode):
    info = zipfile.ZipInfo(name, date_time=stamp())
    info.compress_type = zipfile.ZIP_DEFLATED
    # Unix mode bits in the high half, and "made on Unix", so an unzip on
    # a Mac or Linux box keeps the exes executable; Windows ignores both.
    info.external_attr = (0o100000 | mode) << 16
    info.create_system = 3
    return info


def main(argv):
    if len(argv) != 5:
        sys.stderr.write(__doc__.split("\n\n")[1] + "\n")
        return 2
    player, stub, icon, out = argv[1:]
    # In this order, every time: the one to click first.
    entries = [
        ("mStream Player.exe", stub, 0o755),
        ("mstream-player.exe", player, 0o755),
        ("mstream-player.ico", icon, 0o644),
        ("README.txt", None, 0o644),
    ]
    os.makedirs(os.path.dirname(os.path.abspath(out)), exist_ok=True)
    with zipfile.ZipFile(out, "w") as z:
        for name, src, mode in entries:
            if src is None:
                data = README.encode("utf-8")
            else:
                with open(src, "rb") as f:
                    data = f.read()
            z.writestr(member(name, mode), data)
    print(out)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
