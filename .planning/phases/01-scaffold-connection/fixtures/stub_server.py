#!/usr/bin/env python3
"""Loopback stub IMAP server that accepts every LOGIN.

Fixture generator for SGE Phase 1 (01-02-PLAN, local-simulated fallback):
real client bytes over a real socket, stub answers. NEVER logs the LOGIN
line (it carries the password). Single connection, then exits.

Usage: python3 stub_server.py [port]   (default 11430)
"""

import socket
import sys

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 11430

CAPS = "IMAP4rev1 IDLE NAMESPACE LITERAL+ AUTH=PLAIN"


def handle(conn: socket.socket) -> None:
    f = conn.makefile("rwb")
    f.write(b"* OK [CAPABILITY " + CAPS.encode() + b"] stub ready.\r\n")
    f.flush()
    while True:
        raw = f.readline()
        if not raw:
            return
        line = raw.decode("utf-8", "replace").strip()
        if not line:
            continue
        parts = line.split()
        tag, cmd = parts[0], parts[1].upper()
        # Never print the LOGIN line: it contains the password.
        if cmd != "LOGIN":
            print(f"C: {line}", flush=True)

        def ok(tag: str, text: str = "done") -> None:
            f.write(f"{tag} OK {text}.\r\n".encode())
            f.flush()

        if cmd == "CAPABILITY":
            f.write(f"* CAPABILITY {CAPS}\r\n".encode())
            f.flush()
            ok(tag, "capabilities listed")
        elif cmd == "LOGIN":
            ok(tag, "logged in")
        elif cmd == "NAMESPACE":
            f.write(b'* NAMESPACE (("" "/")) NIL NIL\r\n')
            f.flush()
            ok(tag, "namespace listed")
        elif cmd == "LIST":
            f.write(b'* LIST (\\HasNoChildren) "/" "INBOX"\r\n')
            f.flush()
            ok(tag, "list completed")
        elif cmd == "STATUS":
            f.write(b"* STATUS INBOX (MESSAGES 3 UIDVALIDITY 12345 UIDNEXT 4)\r\n")
            f.flush()
            ok(tag, "status completed")
        elif cmd == "SELECT":
            f.write(b"* 3 EXISTS\r\n* 0 RECENT\r\n")
            f.write(b"* OK [UNSEEN 1] first unseen\r\n")
            f.write(b"* OK [UIDVALIDITY 12345] UIDs valid\r\n")
            f.write(b"* OK [UIDNEXT 4] next UID\r\n")
            f.write(b"* FLAGS (\\Answered \\Flagged \\Deleted \\Seen \\Draft)\r\n")
            f.flush()
            ok(tag, "[READ-WRITE] select completed")
        elif cmd == "LOGOUT":
            f.write(b"* BYE stub closing\r\n")
            f.flush()
            ok(tag, "logout completed")
            return
        else:
            f.write(f"{tag} BAD unknown command in stub\r\n".encode())
            f.flush()


def main() -> None:
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("127.0.0.1", PORT))
    srv.listen(1)
    print(f"stub listening on 127.0.0.1:{PORT}", flush=True)
    conn, _ = srv.accept()
    with conn:
        handle(conn)
    print("stub done", flush=True)


if __name__ == "__main__":
    main()
