#!/usr/bin/env python3
"""Build reproducible pira_nav binaries without building pira_ctx."""

from __future__ import annotations

import sys
import subprocess

import build_pira_ctx_platform_bins as builder


if __name__ == "__main__":
    try:
        builder.configure_tool("pira_nav", uses_c_compiler=True)
        raise SystemExit(builder.main())
    except (builder.BuildError, subprocess.CalledProcessError) as error:
        print(f"build_pira_nav_platform_bins.py: {error}", file=sys.stderr)
        raise SystemExit(1)
