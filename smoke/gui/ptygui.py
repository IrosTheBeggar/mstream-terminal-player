#!/usr/bin/env python3
"""Drive `mstream-player gui` on a pty: answer its terminal queries, keep a
pyte screen, click cells and send keys, wait for text. The scenario is the
module `scenario_<name>` beside this file; see README.md for the rig.

    python3 -u ptygui.py direct
    python3 -u ptygui.py mixed
"""
import os, pty, sys, time, select, signal, fcntl, termios, struct, re
import pyte

COLS, ROWS = 120, 34

class Gui:
    def __init__(self, argv, env, log):
        self.screen = pyte.Screen(COLS, ROWS)
        self.stream = pyte.ByteStream(self.screen)
        self.log = open(log, "ab")
        pid, fd = pty.fork()
        if pid == 0:
            os.environ.clear(); os.environ.update(env)
            os.execvp(argv[0], argv)
        self.pid, self.fd = pid, fd
        fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
        fl = fcntl.fcntl(fd, fcntl.F_GETFL); fcntl.fcntl(fd, fcntl.F_SETFL, fl | os.O_NONBLOCK)
        self.pending = b""

    def answer(self, data):
        # Terminal queries the app makes at startup — per the smoke notes.
        out = b""
        if b"\x1b[c" in data or b"\x1b[0c" in data: out += b"\x1b[?1;2c"
        if b"\x1b[5n" in data: out += b"\x1b[0n"
        if b"\x1b]11;?" in data: out += b"\x1b]11;rgb:1a1a/1a1a/1a1a\x1b\\"
        if b"\x1b[16t" in data: out += b"\x1b[6;20;10t"
        if b"\x1b[6n" in data: out += b"\x1b[1;1R"
        if out:
            os.write(self.fd, out)

    def pump(self, secs):
        end = time.time() + secs
        while True:
            left = end - time.time()
            if left <= 0: return
            r, _, _ = select.select([self.fd], [], [], min(left, 0.05))
            if r:
                try:
                    data = os.read(self.fd, 65536)
                except OSError:
                    return
                if not data: return
                self.log.write(data); self.log.flush()
                self.answer(data)
                self.stream.feed(data)

    def text(self):
        return [self.screen.display[i].rstrip() for i in range(ROWS)]

    def dump(self, title):
        print(f"───── {title} ─────", flush=True)
        for i, line in enumerate(self.text()):
            if line.strip(): print(f"{i:2} {line}", flush=True)

    def find(self, needle, start_row=0):
        for i in range(start_row, ROWS):
            j = self.screen.display[i].find(needle)
            if j >= 0: return i, j
        return None

    def wait_for(self, needle, timeout, what=None):
        end = time.time() + timeout
        while time.time() < end:
            self.pump(0.2)
            hit = self.find(needle)
            if hit: return hit
        self.dump(f"timeout waiting for {what or needle!r}")
        raise SystemExit(f"FAIL: never saw {needle!r}")

    def click(self, row, col):
        # SGR mouse press + release, 1-based cells.
        os.write(self.fd, f"\x1b[<0;{col+1};{row+1}M".encode()); self.pump(0.05)
        os.write(self.fd, f"\x1b[<0;{col+1};{row+1}m".encode()); self.pump(0.2)

    def click_text(self, needle, timeout=10, offset=0):
        row, col = self.wait_for(needle, timeout)
        self.click(row, col + offset)
        return row, col

    def key(self, s):
        os.write(self.fd, s.encode() if isinstance(s, str) else s); self.pump(0.15)

    def quit(self):
        self.key("q")
        end = time.time() + 5
        while time.time() < end:
            self.pump(0.02)
            pid, status = os.waitpid(self.pid, os.WNOHANG)
            if pid: return status
        os.kill(self.pid, signal.SIGKILL); os.waitpid(self.pid, 0)
        return -9

def main():
    sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
    scenario = sys.argv[1]
    mod = __import__(f"scenario_{scenario}")
    mod.run(Gui)

if __name__ == "__main__":
    main()
