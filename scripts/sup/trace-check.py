#!/usr/bin/env python3
"""L-702 (TST-SUP-002): check strace output of searchdump against an allow-list.

    trace-check.py --allow FILE --case NAME --var KEY=VALUE ... TRACE_PREFIX
    trace-check.py --selftest

TRACE_PREFIX is the `-o` of `strace -ff -y -s 0 -e trace=%file,%network,%process`: one file
per thread, TRACE_PREFIX.<tid>. Every traced syscall becomes an *entry*:

    exec PATH          execve/execveat of PATH (the first one is the binary itself)
    path PATH          a path argument of a %file syscall (opened, stat-ed, probed, linked, ...),
                       made absolute against the start directory or the dirfd and normalised;
                       attempts that fail (ENOENT probes) count as well
    connect ADDR       connect/sendto/sendmsg to 127.0.0.1:PORT, [::1]:PORT, unix:PATH or
                       netlink:pid=N
    bind ADDR          bind() to an address (a listening socket would show up here)
    socket FAMILY/TYPE socket() calls
    process fork       clone/clone3/fork/vfork without CLONE_THREAD (a child process)
    syscall NAME       any other traced syscall (thread start, exit, sockopts, ...)

Values are rewritten with the --var placeholders (longest value first), e.g. the work
directory becomes @WORK and the cluster port @PORT. The allow-list holds one rule per line,
`KIND PATTERN  # reason`, PATTERN a glob over the rewritten value where only `*` (any run of
characters, `/` included) and `?` are special (`[::1]` is literal).
Entries no rule matches are the allow-list diff: they are printed as `+ KIND VALUE` and make
the exit status 1. A new path, address or child process therefore needs a reviewed change
to the allow-list file.
"""
import argparse
import glob
import os
import re
import sys

# path argument positions per %file syscall (0-based); a dirfd argument just before an
# `at` path is resolved through strace -y's `N</path>` decoration
PATH_ARGS = {
    "open": [0], "creat": [0], "stat": [0], "lstat": [0], "access": [0], "readlink": [0],
    "mkdir": [0], "rmdir": [0], "unlink": [0], "chdir": [0], "chmod": [0], "chown": [0],
    "lchown": [0], "truncate": [0], "statfs": [0], "utime": [0], "utimes": [0], "chroot": [0],
    "getxattr": [0], "lgetxattr": [0], "setxattr": [0], "lsetxattr": [0], "listxattr": [0],
    "llistxattr": [0], "removexattr": [0], "lremovexattr": [0], "mknod": [0], "acct": [0],
    "swapon": [0], "swapoff": [0], "pivot_root": [0, 1], "mount": [0, 1], "umount2": [0],
    "uselib": [0], "execve": [0],
    "rename": [0, 1], "link": [0, 1], "symlink": [1],
    "openat": [1], "openat2": [1], "newfstatat": [1], "fstatat64": [1], "statx": [1],
    "faccessat": [1], "faccessat2": [1], "readlinkat": [1], "mkdirat": [1], "unlinkat": [1],
    "fchmodat": [1], "fchmodat2": [1], "fchownat": [1], "utimensat": [1], "futimesat": [1],
    "mknodat": [1], "name_to_handle_at": [1], "execveat": [1], "inotify_add_watch": [1],
    "fanotify_mark": [4], "open_tree": [1], "move_mount": [1, 3], "fsconfig": [],
    "linkat": [1, 3], "renameat": [1, 3], "renameat2": [1, 3], "symlinkat": [2],
    "quotactl": [1], "getcwd": [],
}
AT_SYSCALLS = {n for n, a in PATH_ARGS.items() if a and a[0] == 1 or n.endswith("at")}
NET_ADDR = {"connect": 1, "sendto": 4, "sendmsg": 1}
LINE = re.compile(r"^(?P<name>[a-z_0-9]+)\((?P<args>.*)\)\s+=\s+(?P<ret>.*)$")


def split_args(s):
    """Split a strace argument list at top-level commas (quotes, {}, [], () and <> nest)."""
    out, depth, cur, i, q = [], 0, [], 0, False
    while i < len(s):
        c = s[i]
        if q:
            cur.append(c)
            if c == "\\" and i + 1 < len(s):
                cur.append(s[i + 1])
                i += 1
            elif c == '"':
                q = False
        elif c == '"':
            q = True
            cur.append(c)
        elif c in "{[(<":
            depth += 1
            cur.append(c)
        elif c in "}])>":
            depth -= 1
            cur.append(c)
        elif c == "," and depth == 0:
            out.append("".join(cur).strip())
            cur = []
        else:
            cur.append(c)
        i += 1
    if cur or out:
        out.append("".join(cur).strip())
    return out


def unquote(a):
    m = re.match(r'^"((?:[^"\\]|\\.)*)"(\.\.\.)?$', a)
    if not m:
        return None
    raw = m.group(1)
    return bytes(raw, "utf-8").decode("unicode_escape").encode("latin-1").decode("utf-8", "replace")


def fd_path(a):
    m = re.match(r"^-?\d+<(.*)>$", a)
    return m.group(1) if m else None


def sockaddr(a):
    if "sa_family=AF_INET6" in a:
        port = re.search(r"sin6_port=htons\((\d+)\)", a)
        addr = re.search(r'inet_pton\(AF_INET6, "([^"]+)"', a)
        return "[%s]:%s" % (addr.group(1) if addr else "?", port.group(1) if port else "?")
    if "sa_family=AF_INET" in a:
        port = re.search(r"sin_port=htons\((\d+)\)", a)
        addr = re.search(r'inet_addr\("([^"]+)"\)', a)
        return "%s:%s" % (addr.group(1) if addr else "?", port.group(1) if port else "?")
    if "sa_family=AF_UNIX" in a:
        p = re.search(r'sun_path=(@?"(?:[^"\\]|\\.)*")', a)
        return "unix:" + (p.group(1).replace('"', "") if p else "?")
    if "sa_family=AF_NETLINK" in a:
        p = re.search(r"nl_pid=(\d+)", a)
        return "netlink:pid=%s" % (p.group(1) if p else "?")
    m = re.search(r"sa_family=(\w+)", a)
    return m.group(1) if m else None


def entries_from_lines(lines, cwd):
    """Yield (kind, value) for each traced syscall line (strace -ff: no pid prefix)."""
    for line in lines:
        line = line.rstrip("\n")
        if not line or line.startswith("+++") or line.startswith("---"):
            continue
        m = LINE.match(line)
        if not m:
            if line.startswith(("exit(", "exit_group(")):
                yield ("syscall", line.split("(")[0])
                continue
            yield ("unparsed", line[:120])
            continue
        name, args = m.group("name"), split_args(m.group("args"))
        if name in ("execve", "execveat"):
            pos = 0 if name == "execve" else 1
            p = unquote(args[pos]) if len(args) > pos else None
            yield ("exec", absolute(p or "?", cwd, args[0] if name == "execveat" else None))
            continue
        if name in ("clone", "clone3", "fork", "vfork"):
            yield ("syscall", name + ":thread") if "CLONE_THREAD" in m.group("args") else ("process", "fork")
            continue
        if name in NET_ADDR:
            pos = NET_ADDR[name]
            a = args[pos] if len(args) > pos else ""
            if name == "sendmsg":
                a = a if "msg_name=" in a else ""
            addr = sockaddr(a) if a and a != "NULL" else None
            yield ("connect", addr) if addr else ("syscall", name)
            continue
        if name == "bind":
            yield ("bind", sockaddr(args[1]) if len(args) > 1 else "?")
            continue
        if name == "socket":
            yield ("socket", "/".join(x.split("|")[0] for x in args[:2]))
            continue
        if name in PATH_ARGS:
            positions = PATH_ARGS[name]
            if not positions:
                yield ("syscall", name)
                continue
            for pos in positions:
                if len(args) <= pos:
                    continue
                p = unquote(args[pos])
                if p is None:
                    yield ("unparsed", line[:120])
                    continue
                if p == "":
                    # AT_EMPTY_PATH: the dirfd itself (already opened, so already checked)
                    continue
                dirfd = args[pos - 1] if (name in AT_SYSCALLS and pos >= 1) else None
                yield ("path", absolute(p, cwd, dirfd))
            continue
        yield ("syscall", name)


def absolute(p, cwd, dirfd):
    if not p.startswith("/"):
        base = cwd
        if dirfd and not dirfd.startswith("AT_FDCWD"):
            base = fd_path(dirfd) or ("@FD(%s)" % dirfd)
        p = os.path.join(base, p)
    return os.path.normpath(p) if not p.startswith("@FD") else p


def rewrite(value, vars_):
    for k, v in sorted(vars_.items(), key=lambda kv: -len(kv[1])):
        if not v:
            continue
        if k == "PORT":
            value = re.sub(r":%s$" % re.escape(v), ":@PORT", value)
        elif value == v or value.startswith(v.rstrip("/") + "/"):
            value = "@" + k + value[len(v.rstrip("/")):]
    return value


def load_rules(path):
    rules = []
    with open(path, encoding="utf-8") as f:
        for n, line in enumerate(f, 1):
            body = line.split("#", 1)[0].strip()
            if not body:
                continue
            parts = body.split(None, 1)
            if len(parts) != 2:
                raise SystemExit("%s:%d: want `KIND PATTERN`" % (path, n))
            rules.append((parts[0], parts[1], n))
    return rules


def glob_re(pattern):
    return re.compile("".join(".*" if c == "*" else "." if c == "?" else re.escape(c) for c in pattern) + r"\Z")


def check(entries, rules):
    """Return (diff, used): entries no rule matches, and the rule lines that matched."""
    diff, used = {}, set()
    compiled = [(k, glob_re(p), n) for k, p, n in rules]
    for kind, value in entries:
        hit = next((r for r in compiled if r[0] == kind and r[1].match(value)), None)
        if hit:
            used.add(hit[2])
        else:
            diff[(kind, value)] = diff.get((kind, value), 0) + 1
    return diff, used


def trace_entries(prefix, cwd, vars_):
    files = sorted(glob.glob(prefix + ".*"))
    if not files:
        raise SystemExit("no trace files %s.*" % prefix)
    out, execs = [], 0
    for fn in files:
        with open(fn, encoding="utf-8", errors="replace") as f:
            for kind, value in entries_from_lines(f, cwd):
                if kind == "exec":
                    execs += 1
                out.append((kind, rewrite(value, vars_)))
    if execs > 1:
        out.append(("exec-count", str(execs)))
    return out


SELFTEST_RULES = [("exec", "@BIN", 1), ("path", "@WORK/*", 2), ("path", "/etc/ssl/certs/*", 3),
                  ("connect", "127.0.0.1:@PORT", 4), ("connect", "[::1]:@PORT", 8), ("syscall", "clone3:thread", 5),
                  ("syscall", "exit*", 6), ("socket", "AF_INET/SOCK_STREAM", 7)]
SELFTEST_VARS = {"BIN": "/opt/b/edrs", "WORK": "/w", "PORT": "19200"}
CLEAN = [
    'execve("/opt/b/edrs", ["/opt/b/edrs", "--input=x"], 0x7ff /* 3 vars */) = 0',
    'openat(AT_FDCWD, "out/a.json", O_WRONLY|O_CREAT, 0666) = 9</w/out/a.json>',
    'openat(9</etc/ssl/certs>, "x.pem", O_RDONLY) = 10</etc/ssl/certs/x.pem>',
    'statx(11</w/in.json>, "", AT_STATX_SYNC_AS_STAT|AT_EMPTY_PATH, STATX_ALL, {}) = 0',
    'linkat(AT_FDCWD, "out/.a.part", AT_FDCWD, "out/a.json", 0) = 0',
    'clone3({flags=CLONE_VM|CLONE_FS|CLONE_FILES|CLONE_SIGHAND|CLONE_THREAD|CLONE_SYSVSEM, child_tid=0x1}, 88) = 42',
    'socket(AF_INET, SOCK_STREAM|SOCK_CLOEXEC|SOCK_NONBLOCK, IPPROTO_TCP) = 10<socket:[1]>',
    'connect(10<socket:[1]>, {sa_family=AF_INET, sin_port=htons(19200), sin_addr=inet_addr("127.0.0.1")}, 16) = -1 EINPROGRESS (Operation now in progress)',
    'connect(10<socket:[1]>, {sa_family=AF_INET6, sin6_port=htons(19200), sin6_flowinfo=htonl(0), inet_pton(AF_INET6, "::1", &sin6_addr), sin6_scope_id=0}, 28) = 0',
    'exit_group(0)                           = ?',
]
PLANTS = {
    "new path": ('openat(AT_FDCWD, "/home/u/.netrc", O_RDONLY) = 3</home/u/.netrc>', ("path", "/home/u/.netrc")),
    "failed probe": ('access("/etc/ld.so.preload", R_OK) = -1 ENOENT (No such file or directory)', ("path", "/etc/ld.so.preload")),
    "dotdot escape": ('openat(AT_FDCWD, "out/../../etc/shadow", O_RDONLY) = -1 EACCES (Permission denied)', ("path", "/etc/shadow")),
    "second exec": ('execve("/bin/sh", ["sh", "-c", "id"], 0x1 /* 3 vars */) = 0', ("exec", "/bin/sh")),
    "child process": ('clone3({flags=CLONE_VM|CLONE_VFORK, exit_signal=SIGCHLD, stack=0x1}, 88) = 77', ("process", "fork")),
    "other port": ('connect(10<socket:[1]>, {sa_family=AF_INET, sin_port=htons(9200), sin_addr=inet_addr("127.0.0.1")}, 16) = -1 ECONNREFUSED (Connection refused)', ("connect", "127.0.0.1:9200")),
    "ipv6 address": ('connect(10<socket:[1]>, {sa_family=AF_INET6, sin6_port=htons(19200), sin6_flowinfo=htonl(0), inet_pton(AF_INET6, "2001:db8::1", &sin6_addr), sin6_scope_id=0}, 28) = 0', ("connect", "[2001:db8::1]:@PORT")),
    "udp dns": ('sendto(11<UDP:[2]>, "\\x12\\x34", 2, MSG_NOSIGNAL, {sa_family=AF_INET, sin_port=htons(53), sin_addr=inet_addr("10.0.0.2")}, 16) = 2', ("connect", "10.0.0.2:53")),
    "unix socket": ('connect(12<socket:[3]>, {sa_family=AF_UNIX, sun_path="/var/run/nscd/socket"}, 110) = -1 ENOENT (No such file or directory)', ("connect", "unix:/var/run/nscd/socket")),
    "class not glob": ('connect(10<socket:[1]>, {sa_family=AF_INET6, sin6_port=htons(19200), sin6_flowinfo=htonl(0), inet_pton(AF_INET6, "::", &sin6_addr), sin6_scope_id=0}, 28) = 0', ("connect", "[::]:@PORT")),
    "bind": ('bind(10<socket:[1]>, {sa_family=AF_INET, sin_port=htons(8080), sin_addr=inet_addr("0.0.0.0")}, 16) = 0', ("bind", "0.0.0.0:8080")),
    "listen": ('listen(10<socket:[1]>, 128) = 0', ("syscall", "listen")),
}


def selftest():
    ok = True
    clean = [(k, rewrite(v, SELFTEST_VARS)) for k, v in entries_from_lines(CLEAN, "/w")]
    diff, _ = check(clean, SELFTEST_RULES)
    print("selftest clean: %s" % ("OK" if not diff else "FAIL %s" % diff))
    ok &= not diff
    for name, (line, want) in PLANTS.items():
        got = [(k, rewrite(v, SELFTEST_VARS)) for k, v in entries_from_lines([line], "/w")]
        diff, _ = check(got, SELFTEST_RULES)
        good = want in diff
        print("selftest plant %-14s: %s %s" % (name, "CAUGHT" if good else "MISSED", sorted(diff)))
        ok &= good
    print("TRACE_SELFTEST %s" % ("OK" if ok else "FAIL"))
    return 0 if ok else 1


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--allow")
    ap.add_argument("--case", default="run")
    ap.add_argument("--cwd", default=os.getcwd())
    ap.add_argument("--var", action="append", default=[])
    ap.add_argument("--observed", help="append the rewritten entries (sorted, unique) to this file")
    ap.add_argument("--selftest", action="store_true")
    ap.add_argument("prefix", nargs="?")
    a = ap.parse_args()
    if a.selftest:
        return selftest()
    vars_ = dict(v.split("=", 1) for v in a.var)
    entries = trace_entries(a.prefix, a.cwd, vars_)
    rules = load_rules(a.allow)
    diff, used = check(entries, rules)
    if a.observed:
        with open(a.observed, "a", encoding="utf-8") as f:
            for k, v in sorted(set(entries)):
                f.write("%s\t%s\t%s\n" % (a.case, k, v))
    for i, ((k, v), n) in enumerate(sorted(diff.items())):
        if i == 40:
            print("+ ... %d more (all entries: --observed file)" % (len(diff) - 40))
            break
        print("+ %s %s   (%dx, case %s)" % (k, v, n, a.case))
    print("TRACE case=%s entries=%d diff=%d %s" % (a.case, len(entries), len(diff), "OK" if not diff else "FAIL"))
    return 1 if diff else 0


if __name__ == "__main__":
    sys.exit(main())
