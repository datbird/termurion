#!/usr/bin/env python3
"""Probe helper: turns on the given DEC modes, then prints each escape sequence it
receives as a hex line. Quits on 'q' or after 5 s with no input."""
import os
import select
import sys
import termios
import tty

ESC = b"\x1b"
modes = sys.argv[1:]
fd = sys.stdin.fileno()
saved = termios.tcgetattr(fd)
tty.setraw(fd)
try:
    for m in modes:
        os.write(1, f"\x1b[?{m}h".encode())
    os.write(1, b"READY\r\n")
    idle = 0
    while idle < 50:
        ready, _, _ = select.select([fd], [], [], 0.1)
        if not ready:
            idle += 1
            continue
        idle = 0
        data = os.read(fd, 1024)
        if data == b"q":
            break
        # One line per sequence, so several events in one read stay separate.
        for part in data.split(ESC):
            if part:
                os.write(1, f"GOT {(ESC + part).hex()}\r\n".encode())
finally:
    for m in modes:
        os.write(1, f"\x1b[?{m}l".encode())
    termios.tcsetattr(fd, termios.TCSADRAIN, saved)
