#!/usr/bin/env python3
"""TST-SUP-001 (L-701): logging DNS sink for egress-denied runs.

Runs inside the run's own network namespace (loopback only) and answers every DNS query
on 127.0.0.1:53 and [::1]:53 (UDP and TCP) with NXDOMAIN, logging one line per query:
    DNS <proto> <qname> <qtype>
The job compares the log against the allow-list; the sink itself never forwards anything.
Usage: egress-sink.py LOGFILE READYFILE
"""
import socket
import struct
import sys
import threading

LOG = open(sys.argv[1], "a", buffering=1)
LOCK = threading.Lock()


def qname(msg):
    i, labels = 12, []
    while i < len(msg) and msg[i] != 0:
        n = msg[i]
        if n & 0xC0:
            break
        labels.append(msg[i + 1:i + 1 + n].decode("ascii", "replace"))
        i += 1 + n
    qtype = struct.unpack("!H", msg[i + 1:i + 3])[0] if i + 3 <= len(msg) else 0
    return ".".join(labels) or ".", qtype, i + 5


def answer(msg):
    name, qtype, end = qname(msg)
    flags = 0x8183  # response, recursion desired+available, NXDOMAIN
    reply = msg[:2] + struct.pack("!HHHHH", flags, 1, 0, 0, 0) + msg[12:end]
    return name, qtype, reply


def log(proto, name, qtype):
    with LOCK:
        LOG.write(f"DNS {proto} {name} {qtype}\n")


def udp(fam, addr):
    s = socket.socket(fam, socket.SOCK_DGRAM)
    s.bind((addr, 53))
    while True:
        msg, peer = s.recvfrom(4096)
        if len(msg) < 12:
            continue
        name, qtype, reply = answer(msg)
        log("udp", name, qtype)
        s.sendto(reply, peer)


def tcp_conn(c):
    try:
        while True:
            hdr = c.recv(2)
            if len(hdr) < 2:
                return
            (n,) = struct.unpack("!H", hdr)
            msg = b""
            while len(msg) < n:
                part = c.recv(n - len(msg))
                if not part:
                    return
                msg += part
            name, qtype, reply = answer(msg)
            log("tcp", name, qtype)
            c.sendall(struct.pack("!H", len(reply)) + reply)
    finally:
        c.close()


def tcp(fam, addr):
    s = socket.socket(fam, socket.SOCK_STREAM)
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    s.bind((addr, 53))
    s.listen(16)
    while True:
        c, _ = s.accept()
        threading.Thread(target=tcp_conn, args=(c,), daemon=True).start()


for fam, addr in ((socket.AF_INET, "127.0.0.1"), (socket.AF_INET6, "::1")):
    for fn in (udp, tcp):
        threading.Thread(target=fn, args=(fam, addr), daemon=True).start()
open(sys.argv[2], "w").close()
threading.Event().wait()
