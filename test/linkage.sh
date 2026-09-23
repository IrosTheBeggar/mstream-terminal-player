#!/bin/sh
# The binary has to start on a box with no display and no GPU stack — the
# NAS and Raspberry Pi installs `serve` runs on. A library in NEEDED that the
# box lacks stops the process before main, whatever it was linked for. So
# nothing that talks to a display or a GPU may be linked: wgpu and winit open
# theirs at run time instead (PLAN.md, Phase 10), and this holds the build to
# that. Prints the whole NEEDED list either way, for the record.
#
# usage: test/linkage.sh <ELF binary>
set -eu

# On its own, so set -e stops here for a missing file or one that is not
# ELF: piped into sed, that failure would read as an empty, passing list.
dynamic=$(readelf -d "$1")
needed=$(echo "$dynamic" | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p')
echo "NEEDED by $1:"
echo "$needed" | sed 's/^/  /'

display='^lib(vulkan|EGL|GL|GLX|GLESv2|OpenGL|X11|X11-xcb|xcb[a-z0-9-]*|Xcursor|Xrandr|Xi|wayland-[a-z]+|xkbcommon[a-z-]*|gbm|drm|udev)\.so'
linked=$(echo "$needed" | grep -E "$display" || true)
if [ -n "$linked" ]; then
    echo "::error::display or GPU libraries linked rather than loaded — the binary would not start without them:"
    echo "$linked" | sed 's/^/  /'
    exit 1
fi
