#!/usr/bin/env bash
# M1 capture research on a Linux CI runner, X11 (docs/capture-research.md): Xvfb at
# 1920x1080 with a moving picture, then FFmpeg's x11grab (xcbgrab) from our LGPL build:
# capture only, and capture + OpenH264 into fragmented MP4. Usage: x11.sh FFMPEG_DIR
set -u
FF="$1"; SECONDS_RUN="${2:-20}"
mkdir -p results
say() { echo "$*"; echo "$*" >> results/summary.md; }

Xvfb :99 -screen 0 1920x1080x24 -nolisten tcp & XVFB=$!
sleep 2
export DISPLAY=:99
# The moving picture (apt's ffplay, only for display).
ffplay -hide_banner -loglevel error -f lavfi -i "testsrc2=size=1280x720:rate=60" -x 1280 -y 720 -noborder & PLAY=$!
sleep 3

say "## Linux X11 capture research ($(lsb_release -ds), $(nproc) logical CPUs)"
say ""
say "### LGPL FFmpeg build"
for d in x11grab pulse alsa v4l2 kmsgrab; do
  if "$FF/ffmpeg" -hide_banner -devices 2>&1 | grep -qw "$d"; then say "- device $d: yes"; else say "- device $d: NO"; fi
done
for e in libopenh264 h264_vaapi aac; do
  if "$FF/ffmpeg" -hide_banner -encoders 2>&1 | grep -qw "$e"; then say "- encoder $e: yes"; else say "- encoder $e: NO"; fi
done
say ""
say "### Runs (CPU = user+sys over wall, % of one core)"
say '```'
run() { # name, then the ffmpeg arguments
  local name="$1"; shift
  /usr/bin/time -f "%e %U %S" -o "results/$name.time" "$@" > "results/$name.out.txt" 2> "results/$name.err.txt"
  read -r wall user sys < "results/$name.time"
  local stats; stats=$(grep -o 'frame=[^\r]*' "results/$name.err.txt" | tail -1)
  local errs; errs=$(grep -iE 'error|cannot|failed|unknown' "results/$name.err.txt" | head -2 | tr '\n' ' ')
  say "$name: wall ${wall}s cpu $(python3 -c "print(round(100*($user+$sys)/$wall,1))")% | $stats $errs"
}
FFX=("$FF/ffmpeg" -hide_banner -y -stats_period 5 -f x11grab -framerate 30 -video_size 1920x1080 -i :99.0 -t "$SECONDS_RUN")
run x11grab-null "${FFX[@]}" -f null -
run x11grab-openh264 "${FFX[@]}" -vf format=yuv420p -c:v libopenh264 -b:v 8M -g 30 \
  -movflags +frag_keyframe+empty_moov+default_base_moof results/x11grab.mp4
# Same capture through the system's (GPL) FFmpeg, in case ours lacks the device; research only.
run x11grab-null-systemffmpeg ffmpeg -hide_banner -y -stats_period 5 -f x11grab -framerate 30 -video_size 1920x1080 -i :99.0 -t "$SECONDS_RUN" -f null -
say '```'
if [ -f results/x11grab.mp4 ]; then
  say "- x11grab.mp4: $("$FF/ffprobe" -v error -count_frames -select_streams v:0 -show_entries stream=codec_name,profile,width,height,pix_fmt,nb_read_frames:format=duration -of compact results/x11grab.mp4)"
fi
kill $PLAY $XVFB 2>/dev/null
cat results/summary.md >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
