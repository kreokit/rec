#!/usr/bin/env bash
# M1 capture research on a Linux CI runner, Wayland (docs/capture-research.md): a headless
# sway session with PipeWire, WirePlumber, pipewire-pulse, xdg-desktop-portal and its wlr
# backend (no chooser: the only output is shared), a moving picture, then lincap's portal
# ScreenCast -> PipeWire frame count and PulseAudio monitor capture.
# Run inside `dbus-run-session`. Usage: wayland.sh LINCAP
set -u
LINCAP="$1"; SECONDS_RUN="${2:-20}"
mkdir -p results
say() { echo "$*"; echo "$*" >> results/summary-wayland.md; }

export XDG_RUNTIME_DIR="$PWD/xdg-runtime"; mkdir -p "$XDG_RUNTIME_DIR"; chmod 700 "$XDG_RUNTIME_DIR"
export XDG_CONFIG_HOME="$PWD/xdg-config"; mkdir -p "$XDG_CONFIG_HOME/sway" "$XDG_CONFIG_HOME/xdg-desktop-portal-wlr" "$XDG_CONFIG_HOME/xdg-desktop-portal"
cat > "$XDG_CONFIG_HOME/sway/config" <<CFG
output HEADLESS-1 resolution 1920x1080
CFG
cat > "$XDG_CONFIG_HOME/xdg-desktop-portal-wlr/config" <<CFG
[screencast]
output_name=HEADLESS-1
chooser_type=none
max_fps=60
CFG
cat > "$XDG_CONFIG_HOME/xdg-desktop-portal/portals.conf" <<CFG
[preferred]
default=wlr;gtk
org.freedesktop.impl.portal.ScreenCast=wlr
CFG

pipewire > results/pipewire.log 2>&1 &
sleep 1
wireplumber > results/wireplumber.log 2>&1 &
pipewire-pulse > results/pipewire-pulse.log 2>&1 &
# GLES on Mesa's software renderer needs a DRM render node (vgem in CI); wlr-screencopy
# found no usable format with pixman, which is the fallback without one.
RENDERER=pixman; ls /dev/dri/renderD* > /dev/null 2>&1 && RENDERER=gles2
echo "sway renderer: $RENDERER" > results/renderer.txt
WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 WLR_RENDERER=$RENDERER WLR_RENDERER_ALLOW_SOFTWARE=1 sway -c "$XDG_CONFIG_HOME/sway/config" > results/sway.log 2>&1 &
for i in $(seq 1 20); do [ -S "$XDG_RUNTIME_DIR/wayland-1" ] && break; sleep 0.5; done
export WAYLAND_DISPLAY=wayland-1 XDG_CURRENT_DESKTOP=sway XDG_SESSION_TYPE=wayland
dbus-update-activation-environment WAYLAND_DISPLAY XDG_CURRENT_DESKTOP XDG_SESSION_TYPE XDG_RUNTIME_DIR XDG_CONFIG_HOME
/usr/libexec/xdg-desktop-portal-wlr -l DEBUG > results/xdpw.log 2>&1 &
sleep 1
/usr/libexec/xdg-desktop-portal -r > results/xdp.log 2>&1 &
sleep 2
SDL_VIDEODRIVER=wayland ffplay -hide_banner -loglevel error -f lavfi -i "testsrc2=size=1280x720:rate=60" -x 1280 -y 720 > results/ffplay.log 2>&1 &
sleep 3

say "## Linux Wayland capture research (headless sway $(sway --version | awk '{print $3}'), $RENDERER renderer, $(pipewire --version | grep -o 'libpipewire [0-9.]*' | head -1))"
say '```'
/usr/bin/time -f "%e %U %S" -o results/portal.time timeout 60 "$LINCAP" portal "$SECONDS_RUN" > results/portal.json 2> results/portal.err.txt
read -r wall user sys < results/portal.time
say "portal: $(cat results/portal.json) | cpu $(python3 -c "print(round(100*($user+$sys)/$wall,1))")% of one core"

# Computer sound: play a tone into the default sink while lincap records its monitor.
ffmpeg -hide_banner -loglevel error -f lavfi -i "sine=frequency=440:duration=30" -ac 2 -ar 48000 results/tone.wav
paplay results/tone.wav & TONE=$!
sleep 0.5
# PulseAudio protocol without a cookie file (a fresh PipeWire-only system), then with
# one (pipewire-pulse wants 256 bytes but never checks them), then native PipeWire.
timeout 60 "$LINCAP" audio pulse 3 > results/audio-pulse-nocookie.json 2> results/audio-pulse-nocookie.err.txt
say "audio (PulseAudio protocol, no cookie): $(cat results/audio-pulse-nocookie.json)"
head -c 256 /dev/urandom > results/pulse-cookie
PULSE_COOKIE="$PWD/results/pulse-cookie" timeout 60 "$LINCAP" audio pulse 3 > results/audio-pulse.json 2> results/audio-pulse.err.txt
say "audio (PulseAudio protocol, with cookie): $(cat results/audio-pulse.json)"
timeout 60 "$LINCAP" audio pipewire 3 > results/audio-pipewire.json 2> results/audio-pipewire.err.txt
say "audio (PipeWire): $(cat results/audio-pipewire.json)"
wait $TONE 2>/dev/null
say '```'
cat results/summary-wayland.md >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
