#!/usr/bin/env bash

readonly DEFAULT_NAME="world"

greet() {
  local name="${1:-$DEFAULT_NAME}"
  printf 'hello %s\n' "$name"
}
