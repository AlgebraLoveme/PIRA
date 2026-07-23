#!/usr/bin/env python3
"""Build reproducible pira_dec binaries without building other PIRA tools."""

from __future__ import annotations

import subprocess
import sys

import build_pira_ctx_platform_bins as builder


if __name__ == "__main__":
    try:
        builder.configure_tool("pira_dec")
        raise SystemExit(builder.main())
    except (builder.BuildError, subprocess.CalledProcessError) as error:
        print(f"build_pira_dec_platform_bins.py: {error}", file=sys.stderr)
        raise SystemExit(1)
