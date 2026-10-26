#!/usr/bin/env bash
# The macOS 12 real-device measurements for docs/capture-research.md. RECORDS THE REAL
# SCREEN, CAMERA AND MICROPHONE: run only with the user's OK. Everything stays in OUT_DIR;
# analyze.py reads only the counter rows and the click times; the recordings are deleted
# afterwards (pass --keep to keep them).
#
#   research/capture/macos/run-real.sh OUT_DIR [permissions|screen|exclude|full|all] [--keep]
#
#   permissions  asks for camera and mic (waits for the answers), opens the screen-recording
#                request, then waits until Screen Recording is allowed for capreal
#   screen       60 s each: ScreenCaptureKit, AVCaptureScreenInput, CGDisplayStream
#   exclude      10 s each: can the counter window be kept out of the capture?
#   full         600 s: the best screen API + camera + mic, with speaker clicks for sync
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
OUT="$1"; STEP="${2:-all}"; KEEP="${3:-}"
SCREEN_API="${SCREEN_API:-sck}" # for "full": set after the screen step
mkdir -p "$OUT"
[ -d "$OUT/capreal.app" ] || "$HERE/make-capreal-app.sh" "$OUT"

run() { # name, capreal arguments...
  local name="$1"; shift
  echo "== $name: $*"
  open -W -n --stdout "$OUT/$name.json" --stderr "$OUT/$name.log" "$OUT/capreal.app" --args --out "$OUT/$name" "$@"
  cat "$OUT/$name.log"
  if ls "$OUT/$name"/*.mp4 > /dev/null 2>&1; then
    python3 -I "$HERE/../analyze.py" "$OUT/$name" > "$OUT/$name.analysis.json" || true
    [ "$KEEP" = "--keep" ] || rm -f "$OUT/$name"/*.mp4
  fi
}

case "$STEP" in permissions|all)
  open -W -n --stdout "$OUT/permissions.json" --stderr "$OUT/permissions.log" "$OUT/capreal.app" --args --request
  cat "$OUT/permissions.json"
  # macOS 12 applies a new Screen Recording permission only to a new process: check anew.
  # (--check quits too fast for `open -W`: wait for its output instead.)
  for i in $(seq 1 120); do
    : > "$OUT/check.json"
    open -n --stdout "$OUT/check.json" --stderr "$OUT/check.log" "$OUT/capreal.app" --args --check
    for j in $(seq 1 20); do [ -s "$OUT/check.json" ] && break; sleep 0.5; done
    grep -q '"screen": "allowed"' "$OUT/check.json" && break
    sleep 5
  done
  cat "$OUT/check.json" ;;
esac
case "$STEP" in screen|all)
  for api in sck avf cgd; do run "screen-$api" --screen "$api" --seconds 60; done ;;
esac
case "$STEP" in exclude|all)
  for api in sck avf cgd; do run "exclude-$api" --screen "$api" --seconds 10 --hide-pattern; done ;;
esac
case "$STEP" in full|all)
  run "full-$SCREEN_API" --screen "$SCREEN_API" --camera --mic --beep --seconds 600 ;;
esac
echo "results in $OUT (*.json, *.analysis.json)"
