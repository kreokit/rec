// KreoKit Rec's macOS recorder (recorder.m), called from Rust (src/native.rs).
//
// One recording = one folder with one fragmented MP4 per source (screen.mp4, camera.mp4,
// mic.mp4), all on the host clock (docs/capture-research.md). The screen is captured at
// capture_fps, the camera at its own fixed fps; for each, one frame is kept per output frame
// slot (1/fps): the newest frame captured before the slot's middle, or the previous picture
// again when nothing new arrived.

#ifndef KR_RECORDER_H
#define KR_RECORDER_H

#include <stddef.h>
#include <stdint.h>

#define KR_SCREEN_AUTO 0 // ScreenCaptureKit on macOS 13+, CGDisplayStream on 12
#define KR_SCREEN_CGDISPLAYSTREAM 1
#define KR_SCREEN_SCREENCAPTUREKIT 2

typedef struct {
    const char *dir;     // existing folder for the files
    int screen;          // 1: record a display
    uint32_t display_id; // CGDirectDisplayID; 0: the main display
    int width, height;   // output size in pixels (even)
    int fps;             // output frame rate
    int capture_fps;     // capture rate (a multiple of fps): one frame kept per output slot
    int screen_api;      // KR_SCREEN_*
    int video_bitrate;   // bits per second
    // Part of the display, in points from its top-left corner; width 0: the whole display.
    // The output size above keeps the region's shape.
    double region_x, region_y, region_width, region_height;
    int mic;             // 1: record a microphone
    const char *mic_id;  // AVCaptureDevice uniqueID; NULL or "": the default microphone
    int camera;                       // 1: record a camera
    const char *camera_id;            // AVCaptureDevice uniqueID; NULL or "": the default camera
    int camera_width, camera_height;  // output size (even; the camera's own shape, kr_cameras_json)
    int camera_bitrate;               // bits per second
    // Tests: generated sources instead of real devices (a frame counter on the screen and
    // the camera, a click every second on the mic, see recorder.m). synthetic_fast runs them as fast as
    // the encoder allows on their own clock (a simulated hour in minutes);
    // synthetic_static freezes the picture for 2 s of every 10 s (nothing changes on screen);
    // synthetic_late_camera sends the camera's first picture 1 s after the start, as a real one.
    int synthetic;
    int synthetic_fast;
    int synthetic_static;
    int synthetic_late_camera;
} KrRecConfig;

typedef struct {
    double elapsed;          // seconds recorded so far, pauses excluded
    int64_t frames;          // output frames written to screen.mp4
    int64_t frames_new;      // ... showing a newly captured picture
    int64_t frames_repeated; // ... repeating the previous picture (nothing new was captured)
    int64_t frames_late;     // captured pictures that arrived after their slot was written
    int64_t frames_dropped;  // output frames the encoder couldn't take in time
    // The same for camera.mp4, plus pictures the system dropped before they reached us.
    int64_t camera_frames, camera_frames_new, camera_frames_repeated, camera_frames_late;
    int64_t camera_frames_dropped, camera_frames_lost;
    int64_t audio_samples;   // samples written to mic.mp4 (48 kHz)
    int64_t audio_filled;    // samples of silence written where the microphone sent nothing
    int64_t audio_dropped;   // samples dropped (overlapping or the encoder couldn't keep up)
    float mic_peak;          // peak of the last 100 ms of the microphone, 0..1
    int paused;
    int failed; // a source or file failed; kr_rec_stop returns the message
} KrRecStatus;

// Starts capturing; the recording itself starts 0.5 s later (sources settle first). Returns
// NULL and a message in `error` on failure.
void *kr_rec_start(const KrRecConfig *config, char *error, size_t error_len);
void kr_rec_pause(void *rec);
void kr_rec_resume(void *rec);
void kr_rec_status(void *rec, KrRecStatus *status);
// Stops, finishes the files and frees `rec`. Writes the final status; returns 0, or -1
// with a message in `error` (the files are still finished as far as possible).
int kr_rec_stop(void *rec, KrRecStatus *status, char *error, size_t error_len);

// The region overlay window (window.m): above the menu bar and the Dock. Main thread only.
void kr_overlay_window(void *ns_window);

// Permissions: 1 allowed, 0 not (yet) allowed.
int kr_screen_access(void);
// Asks once; on macOS 12 the answer only applies after the app restarts.
void kr_request_screen_access(void);
// 0 not asked yet, 1 denied, 2 restricted, 3 allowed.
int kr_mic_access(void);
// Shows the system dialog if not asked yet; returns the answer (blocks until then).
int kr_request_mic_access(void);
// The same for the camera.
int kr_camera_access(void);
int kr_request_camera_access(void);

// Devices, as JSON arrays; free with kr_free.
// Displays: [{"id", "name", "width", "height" (pixels), "main"}]; mics: [{"id", "name", "default"}];
// cameras: [{"id", "name", "default", "width", "height"}], the size being the largest the
// camera delivers at 30 fps up to 720 pixels high.
char *kr_displays_json(void);
char *kr_mics_json(void);
char *kr_cameras_json(void);
void kr_free(char *s);

// The level meter on the setup screen: opens the microphone without recording.
int kr_mic_monitor_start(const char *mic_id, char *error, size_t error_len);
float kr_mic_monitor_peak(void); // peak of the last 100 ms, 0..1
void kr_mic_monitor_stop(void);

// The camera preview on the setup screen: small JPEG pictures (`width` wide, the camera's
// shape) about 15 times a second, without recording. synthetic: the test pattern instead of
// a camera.
int kr_camera_preview_start(const char *camera_id, int width, int height, int synthetic, char *error, size_t error_len);
// The newest picture if it is newer than *seq (then *seq is updated): its length, the bytes in
// *jpeg (free with kr_free). 0 when there is nothing new.
size_t kr_camera_preview_frame(uint64_t *seq, char **jpeg);
void kr_camera_preview_stop(void);

#endif
