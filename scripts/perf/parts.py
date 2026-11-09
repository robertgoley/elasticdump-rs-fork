#!/usr/bin/env python3
"""Identical sequential split-restore wrapper for Rust and Node; timed as one job."""
import argparse
import subprocess
import sys


def main():
    argv = sys.argv[1:]
    split = argv.index("--")
    ap = argparse.ArgumentParser()
    ap.add_argument("--files", nargs="+", required=True)
    a = ap.parse_args(argv[:split])
    command = argv[split + 1:]
    for path in a.files:
        p = subprocess.run(command + ["--input=" + path])
        if p.returncode:
            return p.returncode
    return 0


if __name__ == "__main__":
    sys.exit(main())
