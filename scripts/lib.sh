# shellcheck shell=bash
# shellcheck disable=SC2034
red=$'\e[31m'; green=$'\e[32m'; yellow=$'\e[33m'; bold=$'\e[1m'; reset=$'\e[0m'

info() { printf '%s==>%s %s\n' "$bold" "$reset" "$*"; }
warn() { printf '%s[warn]%s %s\n' "$yellow" "$reset" "$*"; }
err()  { printf '%s[err]%s %s\n' "$red" "$reset" "$*" >&2; }
