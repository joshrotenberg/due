#!/usr/bin/env python3
"""Independent verifier/permission checker used by both embedded languages."""
from pathlib import Path
import os
import sys
import time

mode = sys.argv[1]
Path("checker.pid").write_text(str(os.getpid()))
if mode == "hold":
    time.sleep(30)
elif mode == "permission":
    time.sleep(0.15)
    print("decline")
elif mode == "verify":
    time.sleep(0.08)
    valid = Path("artifact.txt").read_text() == "good"
    print("verified" if valid else "artifact is incorrect")
    sys.exit(0 if valid else 1)
else:
    raise AssertionError(mode)
