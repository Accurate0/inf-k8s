#!/usr/bin/env bash
# usage: retry.sh <attempts> <delay seconds> <command> [args...]
attempts=$1
delay=$2
shift 2
for attempt in $(seq "$attempts"); do
  "$@" && exit 0
  echo "attempt $attempt/$attempts failed: $*" >&2
  [ "$attempt" = "$attempts" ] || sleep "$delay"
done
exit 1
