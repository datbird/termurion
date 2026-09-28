#!/usr/bin/env python3
"""Records iTerm2's conductor.sh talking to a stand-in for the terminal (milestone 14b).

Usage: python3 scripts/record-conductor.py <conductor.sh> <out dir>

Each scenario runs conductor.sh under `sh` in a new pty with a clean environment and a
PATH of chosen tools, then plays iTerm2's side of the pre-framer stage: the line prefix,
then each command encoded as Conductor.swift's encode() and write() do. Everything read
goes to <name>.bin in read order and everything written to <name>.in.bin. The tests read
these files; nothing here runs at test time. Linux or macOS only; never run against a host.
"""

import base64
import getpass
import gzip
import io
import os
import pty
import re
import select
import shutil
import socket
import sys
import tarfile
import tempfile
import time

START = b"\x1bP2000p"
PREFIX = "\x03" + "8C3A5E1F-0000-4000-8000-00000000A11E"
TOOLS = ["stty", "base64", "awk", "tar", "gzip", "mkdir", "head", "grep", "dd", "cat", "rm",
         "mktemp", "tr", "basename", "tty", "env", "sed"]
END_RE = re.compile(rb"\x1b\]135;:end [^\x1b]*\x1b\\")
UNHOOK_RE = re.compile(rb"\x1b\]135;:unhook\x1b\\")
LIMIT = 10.0


class Refused(Exception):
    pass


def b64(data):
    return base64.b64encode(data).decode("ascii")


def encode(command):
    """Conductor.swift encode() for a pre-framer command plus write()'s ending newline."""
    text = "\n".join(b64(line.encode("utf-8")) for line in command.split("\n"))
    chunks = [text[i:i + 128] for i in range(0, len(text), 128)] or [""]
    return ("\n".join(PREFIX + c for c in chunks) + "\n" + "\n").encode("utf-8")


def payload_tgz():
    """A gzip tar of one file hello.txt holding "hi\\n", byte-stable (mtime 0 everywhere)."""
    raw = io.BytesIO()
    with tarfile.open(fileobj=raw, mode="w", format=tarfile.USTAR_FORMAT) as tar:
        info = tarfile.TarInfo("hello.txt")
        info.size = 3
        info.mode = 0o644
        info.mtime = 0
        tar.addfile(info, io.BytesIO(b"hi\n"))
    return gzip.compress(raw.getvalue(), mtime=0)


class Session:
    def __init__(self, root, script):
        self.read = bytearray()
        self.written = bytearray()
        env = {"HOME": os.path.join(root, "home"), "PATH": os.path.join(root, "bin"),
               "USER": "tandem-recorder", "SHELL": "/bin/sh", "TERM": "xterm"}
        pid, fd = pty.fork()
        if pid == 0:
            os.chdir(env["HOME"])
            os.execve("/bin/sh", ["sh", "-c", script], env)
        self.pid, self.fd = pid, fd

    def pump(self, timeout):
        """Reads what is ready within timeout; False once the child has closed the pty."""
        ready, _, _ = select.select([self.fd], [], [], timeout)
        if not ready:
            return True
        try:
            data = os.read(self.fd, 4096)
        except OSError:
            return False
        if not data:
            return False
        self.read += data
        return True

    def wait_for(self, pattern, since, what):
        deadline = time.monotonic() + LIMIT
        while not pattern.search(bytes(self.read[since:])):
            left = deadline - time.monotonic()
            if left <= 0 or not self.pump(min(left, 0.2)):
                if pattern.search(bytes(self.read[since:])):
                    return
                raise Refused("no " + what + " within 10 s")

    def write(self, data):
        self.written += data
        os.write(self.fd, data)

    def command(self, text, until=END_RE):
        mark = len(self.read)
        self.write(encode(text))
        self.wait_for(until, mark, "answer to " + text.split(" ")[0])

    def settle(self, quiet=0.5):
        """Reads until the output has been quiet for a while (a shell's prompt printed)."""
        deadline = time.monotonic() + LIMIT
        while time.monotonic() < deadline:
            before = len(self.read)
            if not self.pump(quiet):
                return
            if len(self.read) == before:
                return
        raise Refused("output never settled")

    def finish(self):
        deadline = time.monotonic() + LIMIT
        while time.monotonic() < deadline:
            if not self.pump(0.2):
                break
        else:
            raise Refused("the child did not exit")
        os.waitpid(self.pid, 0)
        os.close(self.fd)


def fake_python(version):
    return ("#!/bin/sh\n"
            "if [ \"$1\" = \"-V\" ]; then echo \"Python " + version + "\"; "
            "else exec /usr/bin/python3 \"$@\"; fi\n")


def scenario(conductor, python, steps):
    root = tempfile.mkdtemp(prefix="tandem-rec-")
    try:
        os.mkdir(os.path.join(root, "home"))
        bindir = os.path.join(root, "bin")
        os.mkdir(bindir)
        os.symlink("/bin/sh", os.path.join(bindir, "sh"))
        for tool in TOOLS:
            found = shutil.which(tool)
            if not found:
                raise Refused("missing tool " + tool)
            os.symlink(os.path.realpath(found), os.path.join(bindir, tool))
        if python:
            path = os.path.join(bindir, "python3")
            with open(path, "w") as f:
                f.write(fake_python(python))
            os.chmod(path, 0o755)
        arg = lambda s: b64(s.encode("utf-8"))
        script = (conductor + "\nmain " + arg("none") + " " + arg("12345") + " "
                  + arg(arg("46AaCfGgKkMNnqsTtVvXxYy")) + " " + arg(arg("example.com ")))
        s = Session(root, script)
        s.wait_for(re.compile(re.escape(START) + rb"[^\n]*\n"), 0, "DCS 2000p line")
        s.write((PREFIX + "\n").encode("utf-8"))
        for step in steps:
            kind, text = step
            if kind == "cmd":
                s.command(text)
            elif kind == "cmd-unhook":
                s.command(text, UNHOOK_RE)
                s.settle()
            elif kind == "type":
                s.write(text.encode("utf-8"))
                s.settle()
        s.finish()
        scrub = [(root + "/home", "/home/user"), (root, "/home/user"),
                 (getpass.getuser(), "user"), (socket.gethostname(), "host")]
        data = bytes(s.read)
        for old, new in scrub:
            data = data.replace(old.encode("utf-8"), new.encode("utf-8"))
        return data, bytes(s.written)
    finally:
        shutil.rmtree(root, ignore_errors=True)


def main():
    if os.name == "nt" or sys.platform.startswith("win"):
        sys.exit("record-conductor.py runs on Linux or macOS only")
    if len(sys.argv) != 3:
        sys.exit("usage: python3 scripts/record-conductor.py <conductor.sh> <out dir>")
    with open(sys.argv[1], encoding="utf-8") as f:
        conductor = f.read()
    out_dir = sys.argv[2]
    os.makedirs(out_dir, exist_ok=True)
    tgz = payload_tgz()
    scenarios = [
        ("level1-nopython", None, [
            ("cmd", "getshell"),
            ("cmd", "setenv FOO bar"),
            ("cmd", "write " + b64(tgz) + " /$HOME/.t"),
            ("cmd", "cd /tmp"),
            ("cmd", "pythonversion"),
            ("cmd-unhook", "exec_login_shell"),
            ("type", "echo hi\n"),
            ("type", "exit\n"),
        ]),
        ("python36", "3.6.9", [
            ("cmd", "getshell"),
            ("cmd", "pythonversion"),
            ("cmd-unhook", "exec_login_shell"),
            ("type", "exit\n"),
        ]),
        ("python312", "3.12.1", [
            ("cmd", "getshell"),
            ("cmd", "pythonversion"),
            ("cmd", "quit"),
        ]),
        ("quirks", None, [
            ("cmd", "getshell"),
            ("cmd", "setenv A $'x y'"),
            ("cmd", "nosuch"),
            ("cmd-unhook", "run echo ran"),
        ]),
    ]
    # Nothing is written unless every scenario records.
    try:
        results = [(name, scenario(conductor, python, steps)) for name, python, steps in scenarios]
    except Refused as e:
        sys.exit("recording refused: " + str(e))
    with open(os.path.join(out_dir, "payload.tgz"), "wb") as f:
        f.write(tgz)
    for name, (read, written) in results:
        with open(os.path.join(out_dir, name + ".bin"), "wb") as f:
            f.write(read)
        with open(os.path.join(out_dir, name + ".in.bin"), "wb") as f:
            f.write(written)
        print(name + ": " + str(len(read)) + " bytes read, " + str(len(written)) + " written")


if __name__ == "__main__":
    main()
