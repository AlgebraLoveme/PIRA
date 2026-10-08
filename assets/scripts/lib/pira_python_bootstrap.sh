#!/usr/bin/env sh
# Shared Python 3 discovery/bootstrap helpers for PIRA POSIX wrappers.

pira_find_python3() {
  # The audio wrapper keeps its Python 3 requirement; configuration passes 11.
  minimum_minor=${1:-0}
  # PIRA: version-only fallbacks cover 3.11-3.14; extend for newer names if needed.
  for candidate in python3 python python3.14 python3.13 python3.12 python3.11; do
    py=$(command -v "$candidate") || continue
    if "$py" -c "import sys; raise SystemExit(0 if sys.version_info[0] == 3 and sys.version_info >= (3, $minimum_minor) else 1)" >/dev/null 2>&1; then
      printf '%s\n' "$py"
      return 0
    fi
  done
  return 1
}

pira_arg_has_yes() {
  for arg in "$@"; do
    if [ "$arg" = "--yes" ]; then
      return 0
    fi
  done
  return 1
}

pira_python_assume_yes() {
  if [ "${PIRA_SETUP_ASSUME_YES:-0}" = "1" ]; then
    return 0
  fi
  pira_arg_has_yes "$@"
}

pira_offer_python_install() {
  assume_yes=0
  if pira_python_assume_yes "$@"; then
    assume_yes=1
  fi
  os=$(uname -s 2>/dev/null || printf unknown)
  case "$os" in
    Darwin)
      if command -v brew >/dev/null 2>&1; then
        printf '%s\n' "Python 3.11+ was not found. I can install it with Homebrew:" >&2
        printf '%s\n' "  brew install python" >&2
        if [ "$assume_yes" = "1" ]; then
          brew install python >&2
          return $?
        fi
        printf '%s' "Install Python 3.11+ now with Homebrew? [y/N] " >&2
        read ans || ans=
        case "$ans" in
          y|Y|yes|YES) brew install python >&2 ;;
          *) return 1 ;;
        esac
      else
        printf '%s\n' \
          'Python 3.11+ was not found, and Homebrew is not available.' \
          'Install Python 3.11+ first, for example from https://www.python.org/downloads/ or Homebrew, then rerun this setup wrapper.' >&2
        return 1
      fi
      ;;
    Linux)
      printf '%s\n' \
        'Python 3.11+ was not found.' \
        'Install Python 3.11+ with your system package manager, for example:' \
        '  sudo apt-get update && sudo apt-get install -y python3' \
        '  sudo dnf install -y python3' \
        '  sudo pacman -S python' \
        'Package versions vary by distribution: ensure Python is 3.11+ before rerunning this setup wrapper.' >&2
      return 1
      ;;
    *)
      printf '%s\n' \
        'Python 3.11+ was not found.' \
        'Install Python 3.11+ for this platform, then rerun this setup wrapper.' >&2
      return 1
      ;;
  esac
}

pira_require_python3() {
  py=$(pira_find_python3 || true)
  if [ -z "${py:-}" ]; then
    printf '%s\n' 'ERROR: Python 3 is required. Run assets/scripts/setup_pira.sh first, or install Python 3 and retry.' >&2
    return 1
  fi
  printf '%s\n' "$py"
}

pira_bootstrap_python3() {
  py=$(pira_find_python3 11 || true)
  if [ -z "${py:-}" ]; then
    pira_offer_python_install "$@" || true
    py=$(pira_find_python3 11 || true)
  fi
  if [ -z "${py:-}" ]; then
    printf '%s\n' 'ERROR: Python 3.11+ is required for PIRA configuration. Install it and ensure it is on PATH, then rerun this setup wrapper.' >&2
    return 1
  fi
  printf '%s\n' "$py"
}
