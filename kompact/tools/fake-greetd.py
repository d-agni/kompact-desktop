#!/usr/bin/env python3
"""14c: a stand-in for greetd, to try kompact's login screen without installing anything.

It speaks greetd's IPC (native-endian u32 length + JSON) on a Unix socket: create_session asks
one secret question, the answer is right if it equals FAKE_GREETD_PASSWORD (default "kompact";
your real password is never checked or seen), and start_session only prints the command and
environment greetd would run, then the stand-in exits.

    python3 tools/fake-greetd.py /run/user/1000/kompact-fake-greetd.sock
"""

import json
import os
import socket
import struct
import sys

PASSWORD = os.environ.get("FAKE_GREETD_PASSWORD", "kompact")


def log(*a):
    print("fake-greetd:", *a, file=sys.stderr, flush=True)


def read(conn):
    head = conn.recv(4, socket.MSG_WAITALL)
    if len(head) < 4:
        return None
    (n,) = struct.unpack("=I", head)
    body = conn.recv(n, socket.MSG_WAITALL) if n else b""
    return json.loads(body)


def write(conn, msg):
    body = json.dumps(msg).encode()
    conn.sendall(struct.pack("=I", len(body)) + body)


def serve(conn):
    """One greeter connection; True once a session was started."""
    user = None
    asked = False
    while True:
        msg = read(conn)
        if msg is None:
            return False
        kind = msg.get("type")
        if kind == "create_session":
            user = msg.get("username")
            asked = True
            log(f"create_session for {user!r}")
            write(conn, {"type": "auth_message", "auth_message_type": "secret", "auth_message": "Password: "})
        elif kind == "post_auth_message_response":
            if not asked:
                write(conn, {"type": "error", "error_type": "error", "description": "no question was asked"})
                continue
            asked = False
            if msg.get("response") == PASSWORD:
                log(f"right test password for {user!r}")
                write(conn, {"type": "success"})
            else:
                log(f"wrong test password for {user!r}")
                write(conn, {"type": "error", "error_type": "auth_error", "description": "pam_authenticate: AUTH_ERR"})
        elif kind == "start_session":
            log(f"start_session for {user!r}: cmd={msg.get('cmd')} env={msg.get('env')}")
            write(conn, {"type": "success"})
            return True
        elif kind == "cancel_session":
            log("cancel_session")
            user, asked = None, False
            write(conn, {"type": "success"})
        else:
            write(conn, {"type": "error", "error_type": "error", "description": f"unknown request {kind!r}"})


def main():
    if len(sys.argv) != 2:
        sys.exit("usage: fake-greetd.py <socket path>")
    path = sys.argv[1]
    if os.path.exists(path):
        os.unlink(path)
    srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    srv.bind(path)
    os.chmod(path, 0o600)
    srv.listen(4)
    log(f"listening on {path}; the test password is {PASSWORD!r}")
    try:
        while True:
            conn, _ = srv.accept()
            with conn:
                if serve(conn):
                    log("a session would start now; done")
                    return
    finally:
        srv.close()
        os.unlink(path)


if __name__ == "__main__":
    main()
