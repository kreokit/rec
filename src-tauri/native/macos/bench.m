// Measurements on a real Mac through the app's own recorder (recorder.m), built only with
// the `bench` feature. RECORDS THE REAL SCREEN AND MICROPHONE: run only with the user's OK.
// Started as the app bundle (`open -n "KreoKit Rec Test.app" --args --bench ...`) so macOS
// asks for permission for the app itself; `rec --bench-analyze DIR` reads the results.
//
//   --bench fresh --out DIR [--seconds 60] [--api auto|cgd|sck] [--fps 30|60] [--capture-fps 60] [--mic] [--camera] [--region|--window] [--exclude]
//       A small window shows the test pattern of src/testpattern.rs: a counter that steps
//       60 times a second and stands still for 2 s of every 10 s. The recording shows
//       whether every output frame caught the newest picture (fresh), repeated it because
//       nothing changed (unchanged), or missed a change (stale). --camera also records the
//       default camera at 720p (the long run of screen + camera + mic); the process's CPU
//       use is reported too. --region records only a region around the window (24 points
//       on each side) at the display's own pixels, as the app does (the whole display is
//       captured at full size and the region cut out), so reading the counter back checks
//       that the crop is the right part of the screen. --window records the counter window
//       itself as a window (ScreenCaptureKit, macOS 12.3+), at the display's own pixels.
//       --exclude adds a second window just under the counter (inside the region), striped
//       black and white and flipping twice a second, standing in for the app's floating
//       control, and asks the recorder to keep it out: the analysis counts the frames where
//       its stripes show (0 when the exclusion works; CGDisplayStream can't exclude).
//       --fps 60 records the screen at 60 frames a second (capture at 60 too, one frame per
//       slot; 1.5 times the 30 fps bitrate); the camera stays at 30.
//   --bench sync --out DIR [--seconds 45]
//       The tap test for sound against picture. The window waits in front of everything
//       until the user clicks it and presses space; that first press starts the
//       measurement (`seconds` from then). The user presses the space bar firmly;
//       each press makes a click the microphone hears, and the window flashes white at
//       that moment. The offsets between the flashes and the clicks in the recording are
//       the sync of the two tracks (plus the screen's own delay to show the flash).
//   --bench camera --out DIR [--seconds 45]
//       The clap test for the camera against the microphone: camera (720p) + mic only,
//       starting 3 s after launch. The user claps about once a second in view of the
//       camera; the analysis finds each clap in the sound and the hands meeting in the
//       picture (the largest change from one frame to the next).

#include <sys/resource.h>

#include "recorder.h"

// CPU seconds this process has used (all threads, user + system).
static double cpuSeconds(void) {
    struct rusage u;
    getrusage(RUSAGE_SELF, &u);
    return u.ru_utime.tv_sec + u.ru_utime.tv_usec / 1e6 + u.ru_stime.tv_sec + u.ru_stime.tv_usec / 1e6;
}

#import <AppKit/AppKit.h>
#import <CoreMedia/CoreMedia.h>

static double hostNow(void) { return CMTimeGetSeconds(CMClockGetTime(CMClockGetHostTimeClock())); }

static double gZero;      // the pattern's counter 0, host seconds
static const int kRate = 60; // counter steps per second

static BOOL stillAt(double since) {
    double phase = fmod(since, 10.0);
    return since >= 0 && phase >= 5 && phase < 7;
}

@interface KRPatternView : NSView
@end
@implementation KRPatternView
- (BOOL)isFlipped { return YES; }
- (void)drawRect:(NSRect)dirty {
    CGFloat cell = self.bounds.size.width / 34;
    double since = hostNow() - gZero;
    int64_t n = since < 0 ? 0 : (int64_t)floor(since * kRate);
    [[NSColor colorWithWhite:0.5 alpha:1] setFill];
    NSRectFill(self.bounds);
    for (int b = 0; b < 32; b++) {
        [((n >> (31 - b)) & 1) ? NSColor.whiteColor : NSColor.blackColor setFill];
        NSRectFill(NSMakeRect(cell / 2 + b * cell, cell / 2, cell, cell));
    }
    [(n % kRate < 2) ? NSColor.whiteColor : NSColor.blackColor setFill];
    NSRectFill(NSMakeRect(0, cell * 2, self.bounds.size.width, cell * 2));
}
@end

// The stand-in for the app's own controls: 8-pixel black and white stripes, flipping
// twice a second.
@interface KRStripeView : NSView
@property(nonatomic) CGFloat stripe; // points
@end
@implementation KRStripeView
- (void)drawRect:(NSRect)dirty {
    BOOL flip = (int64_t)floor(hostNow() * 2) % 2;
    for (int i = 0; i * self.stripe < self.bounds.size.width; i++) {
        [((i % 2) ^ flip) ? NSColor.whiteColor : NSColor.blackColor setFill];
        NSRectFill(NSMakeRect(i * self.stripe, 0, self.stripe, self.bounds.size.height));
    }
}
@end

@interface KRFlashView : NSView
@property(nonatomic) double flashUntil;
@property(nonatomic) NSMutableArray<NSNumber *> *presses; // host seconds
@property(nonatomic, copy) void (^onFirstPress)(void);
@property(nonatomic) BOOL measuring;
@end
@implementation KRFlashView
- (BOOL)acceptsFirstResponder { return YES; }
- (void)drawRect:(NSRect)dirty {
    BOOL on = hostNow() < self.flashUntil;
    [on ? NSColor.whiteColor : NSColor.blackColor setFill];
    NSRectFill(self.bounds);
    if (!on) {
        // Big while waiting; small and out of the measured middle while measuring.
        NSString *text = self.measuring ? @"Measuring: keep pressing space firmly, about once a second." : @"Click here, then press space once a second";
        NSDictionary *attrs = @{
            NSForegroundColorAttributeName : self.measuring ? [NSColor colorWithWhite:0.6 alpha:1] : NSColor.whiteColor,
            NSFontAttributeName : self.measuring ? [NSFont systemFontOfSize:15] : [NSFont boldSystemFontOfSize:30],
        };
        NSSize size = [text sizeWithAttributes:attrs];
        CGFloat y = self.measuring ? 24 : (self.bounds.size.height - size.height) / 2;
        [text drawAtPoint:NSMakePoint((self.bounds.size.width - size.width) / 2, y) withAttributes:attrs];
    }
}
- (void)keyDown:(NSEvent *)event {
    if (event.isARepeat || ![event.characters isEqual:@" "]) return;
    // NSEvent timestamps are seconds since startup on the host clock.
    [self.presses addObject:@(event.timestamp)];
    if (!self.measuring) {
        self.measuring = YES;
        if (self.onFirstPress) self.onFirstPress();
    }
    self.flashUntil = hostNow() + 0.15;
    self.needsDisplay = YES;
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 160 * NSEC_PER_MSEC), dispatch_get_main_queue(), ^{ self.needsDisplay = YES; });
}
@end

static void writeJson(NSString *path, id object) {
    NSData *data = [NSJSONSerialization dataWithJSONObject:object options:NSJSONWritingPrettyPrinted error:nil];
    [data writeToFile:path atomically:YES];
}

static NSDictionary *statusJson(const KrRecStatus *s) {
    return @{
        @"elapsed" : @(s->elapsed), @"frames" : @(s->frames), @"frames_new" : @(s->frames_new), @"frames_repeated" : @(s->frames_repeated),
        @"frames_late" : @(s->frames_late), @"frames_dropped" : @(s->frames_dropped), @"audio_samples" : @(s->audio_samples),
        @"audio_filled" : @(s->audio_filled), @"audio_dropped" : @(s->audio_dropped), @"failed" : s->failed ? @YES : @NO,
        @"camera_frames" : @(s->camera_frames), @"camera_frames_new" : @(s->camera_frames_new),
        @"camera_frames_repeated" : @(s->camera_frames_repeated), @"camera_frames_late" : @(s->camera_frames_late),
        @"camera_frames_dropped" : @(s->camera_frames_dropped), @"camera_frames_lost" : @(s->camera_frames_lost),
    };
}

// The recorder's output size for the main display at 1080p (or the display's own size if
// smaller), keeping its shape, as the app does (src/sessions.rs).
static void outputSize(int *w, int *h) {
    CGDisplayModeRef mode = CGDisplayCopyDisplayMode(CGMainDisplayID());
    double pw = CGDisplayModeGetPixelWidth(mode), ph = CGDisplayModeGetPixelHeight(mode);
    CGDisplayModeRelease(mode);
    double scale = MIN(1.0, 1080.0 / ph);
    *w = ((int)round(pw * scale)) & ~1;
    *h = ((int)round(ph * scale)) & ~1;
}

int kr_bench_main(int argc, const char **argv) {
    @autoreleasepool {
        NSString *mode = nil, *out = nil, *api = @"auto";
        double seconds = 0;
        int captureFps = 60, fps = 30;
        BOOL mic = NO, camera = NO, region = NO, windowCapture = NO, exclude = NO;
        for (int i = 1; i < argc; i++) {
            const char *a = argv[i], *v = i + 1 < argc ? argv[i + 1] : "";
            if (!strcmp(a, "--bench")) mode = @(v), i++;
            else if (!strcmp(a, "--out")) out = @(v), i++;
            else if (!strcmp(a, "--seconds")) seconds = atof(v), i++;
            else if (!strcmp(a, "--api")) api = @(v), i++;
            else if (!strcmp(a, "--capture-fps")) captureFps = atoi(v), i++;
            else if (!strcmp(a, "--fps")) fps = atoi(v), i++;
            else if (!strcmp(a, "--mic")) mic = YES;
            else if (!strcmp(a, "--camera")) camera = YES;
            else if (!strcmp(a, "--region")) region = YES;
            else if (!strcmp(a, "--window")) windowCapture = YES;
            else if (!strcmp(a, "--exclude")) exclude = YES;
        }
        if ([mode isEqual:@"check"]) {
            // What this app may record, and the devices it sees (no recording).
            char *displays = kr_displays_json(), *mics = kr_mics_json(), *cameras = kr_cameras_json();
            printf("{\"screen\": %d, \"mic\": %d, \"camera\": %d, \"displays\": %s, \"mics\": %s, \"cameras\": %s}\n", kr_screen_access(),
                   kr_mic_access(), kr_camera_access(), displays, mics, cameras);
            kr_free(displays);
            kr_free(mics);
            kr_free(cameras);
            return 0;
        }
        BOOL sync = [mode isEqual:@"sync"], clap = [mode isEqual:@"camera"];
        if (!out || !(sync || clap || [mode isEqual:@"fresh"])) {
            fprintf(stderr, "usage: --bench fresh|sync|camera --out DIR [--seconds N] [--api auto|cgd|sck] [--fps 30|60] [--capture-fps N] [--mic] [--camera] [--region|--window] [--exclude]\n");
            return 2;
        }
        if (seconds <= 0) seconds = sync || clap ? 45 : 60;
        [[NSFileManager defaultManager] createDirectoryAtPath:out withIntermediateDirectories:YES attributes:nil error:nil];
        NSApplication *app = NSApplication.sharedApplication;
        app.activationPolicy = sync ? NSApplicationActivationPolicyRegular : NSApplicationActivationPolicyAccessory;

        int w, h;
        outputSize(&w, &h);
        NSScreen *screen = NSScreen.screens.firstObject; // the main display (menu bar)
        CGFloat scale = w / screen.frame.size.width;     // output pixels per point
        NSWindow *window;
        NSMutableDictionary *info = [NSMutableDictionary dictionaryWithDictionary:@{@"mode" : mode, @"width" : @(w), @"height" : @(h), @"capture_fps" : @(captureFps), @"api" : api}];
        KRFlashView *flash = nil;
        CGRect area = CGRectZero; // --region, display points from the top-left
        NSWindow *excluded = nil;  // --exclude
        if (clap) {
            window = nil;
            camera = YES;
            mic = YES;
        } else if (sync) {
            NSRect frame = NSMakeRect(0, 0, 760, 440);
            window = [[NSWindow alloc] initWithContentRect:frame styleMask:NSWindowStyleMaskTitled backing:NSBackingStoreBuffered defer:NO];
            window.title = @"KreoKit Rec: sync test";
            window.level = NSStatusWindowLevel; // in front of everything
            [window center];
            flash = [[KRFlashView alloc] initWithFrame:frame];
            flash.presses = [NSMutableArray array];
            window.contentView = flash;
            [window makeKeyAndOrderFront:nil];
            [window orderFrontRegardless];
            [window makeFirstResponder:flash];
            [app activateIgnoringOtherApps:YES];
            NSRect content = [window convertRectToScreen:[flash convertRect:flash.bounds toView:nil]];
            CGFloat top = NSMaxY(screen.frame) - NSMaxY(content);
            // The middle of the window (the instructions are drawn near its bottom).
            info[@"flash"] = @[ @((content.origin.x - screen.frame.origin.x + 40) * scale), @(top * scale + 40 * scale), @((content.size.width - 80) * scale), @((content.size.height - 120) * scale) ];
            mic = YES;
        } else {
            // 16 output pixels per counter square.
            CGFloat cellPt = round(16 / scale * 4) / 4;
            NSRect vis = screen.visibleFrame;
            NSRect frame = NSMakeRect(vis.origin.x + 40, NSMaxY(vis) - 40 - cellPt * 4, cellPt * 34, cellPt * 4);
            window = [[NSWindow alloc] initWithContentRect:frame styleMask:NSWindowStyleMaskBorderless backing:NSBackingStoreBuffered defer:NO];
            window.level = NSStatusWindowLevel;
            KRPatternView *view = [[KRPatternView alloc] initWithFrame:NSMakeRect(0, 0, frame.size.width, frame.size.height)];
            window.contentView = view;
            [window orderFrontRegardless];
            gZero = hostNow() + 1.0;
            // Redrawn at every display refresh, except while the pattern stands still.
            [NSTimer scheduledTimerWithTimeInterval:1.0 / 120 repeats:YES block:^(NSTimer *t) {
                if (!stillAt(hostNow() - gZero)) view.needsDisplay = YES;
            }];
            CGFloat top = NSMaxY(screen.frame) - NSMaxY(frame);
            CGFloat left = frame.origin.x - screen.frame.origin.x;
            CGPoint exclusion = CGPointZero; // its top-left, display points from the top-left
            CGFloat stripePt = 8 / scale;
            if (exclude) {
                // 4 points under the counter, 16 high: inside --region's 24-point margin.
                NSRect exFrame = NSMakeRect(frame.origin.x, frame.origin.y - 20, frame.size.width, 16);
                excluded = [[NSWindow alloc] initWithContentRect:exFrame styleMask:NSWindowStyleMaskBorderless backing:NSBackingStoreBuffered defer:NO];
                excluded.level = NSStatusWindowLevel;
                KRStripeView *stripes = [[KRStripeView alloc] initWithFrame:NSMakeRect(0, 0, exFrame.size.width, exFrame.size.height)];
                stripes.stripe = stripePt;
                excluded.contentView = stripes;
                [excluded orderFrontRegardless];
                [NSTimer scheduledTimerWithTimeInterval:0.25 repeats:YES block:^(NSTimer *t) { stripes.needsDisplay = YES; }];
                exclusion = CGPointMake(left, top + frame.size.height + 4);
                info[@"exclude_window"] = @(excluded.windowNumber);
            }
            if (region) {
                // At the display's own pixels per point, as sessions.rs's source_size does
                // for a region smaller than 1080p.
                CGFloat m = 24;
                area = CGRectMake(left - m, top - m, frame.size.width + 2 * m, frame.size.height + 2 * m);
                scale = screen.backingScaleFactor;
                w = (int)(area.size.width * scale) & ~1;
                h = (int)(area.size.height * scale) & ~1;
                info[@"width"] = @(w);
                info[@"height"] = @(h);
                info[@"region"] = @[ @(area.origin.x), @(area.origin.y), @(area.size.width), @(area.size.height) ];
                left = m;
                top = m;
            }
            if (windowCapture) {
                // The window alone, at the display's own pixels: the pattern fills it.
                scale = screen.backingScaleFactor;
                w = (int)(frame.size.width * scale) & ~1;
                h = (int)(frame.size.height * scale) & ~1;
                info[@"width"] = @(w);
                info[@"height"] = @(h);
                info[@"window"] = @(window.windowNumber);
                left = 0;
                top = 0;
            }
            if (excluded) {
                CGPoint origin = region ? area.origin : CGPointZero;
                // Its middle row in output pixels, and the stripe width there.
                info[@"excluded"] = @{
                    @"x" : @((exclusion.x - origin.x) * scale), @"y" : @((exclusion.y - origin.y + 8) * scale),
                    @"width" : @(excluded.frame.size.width * scale), @"stripe" : @(stripePt * scale),
                };
            }
            info[@"pattern"] = @{@"x" : @(left * scale), @"y" : @(top * scale), @"cell" : @(cellPt * scale), @"rate" : @(kRate), @"still_periods" : @YES};
        }

        KrRecConfig cfg = {0};
        cfg.dir = out.fileSystemRepresentation;
        cfg.screen = 1;
        cfg.width = w;
        cfg.height = h;
        if (fps != 30 && fps != 60) fps = 30;
        if (captureFps < fps) captureFps = fps;
        cfg.fps = fps;
        info[@"fps"] = @(fps);
        info[@"capture_fps"] = @(captureFps);
        cfg.capture_fps = captureFps;
        cfg.screen_api = [api isEqual:@"cgd"] ? KR_SCREEN_CGDISPLAYSTREAM : [api isEqual:@"sck"] ? KR_SCREEN_SCREENCAPTUREKIT : KR_SCREEN_AUTO;
        cfg.video_bitrate = (int)(w * h * 30 * 0.13 * (fps == 60 ? 1.5 : 1.0));
        cfg.region_x = area.origin.x;
        cfg.region_y = area.origin.y;
        cfg.region_width = area.size.width;
        cfg.region_height = area.size.height;
        cfg.window_id = windowCapture ? (uint32_t)window.windowNumber : 0;
        static uint32_t excludeIds[1]; // read when the recording starts (after this returns)
        excludeIds[0] = excluded ? (uint32_t)excluded.windowNumber : 0;
        cfg.exclude_windows = excluded ? excludeIds : NULL;
        cfg.exclude_count = excluded ? 1 : 0;
        cfg.mic = mic;
        cfg.screen = !clap;
        cfg.camera = camera;
        cfg.camera_width = 1280;
        cfg.camera_height = 720;
        cfg.camera_bitrate = (int)(1280 * 720 * 30 * 0.13);
        info[@"camera"] = @(camera);
        __block void *rec = NULL;
        __block double t0 = 0, cpu0 = 0, wall0 = 0;
        void (^begin)(void) = ^{
            char error[1024] = {0};
            KrRecConfig c = cfg;
            rec = kr_rec_start(&c, error, sizeof error);
            if (!rec) {
                fprintf(stderr, "%s\n", error);
                writeJson([out stringByAppendingPathComponent:@"bench.json"], @{@"error" : @(error)});
                exit(1);
            }
            fprintf(stderr, "recording %.0f s into %s\n", seconds, out.UTF8String);
            cpu0 = cpuSeconds();
            wall0 = hostNow();
            dispatch_after(dispatch_time(DISPATCH_TIME_NOW, NSEC_PER_SEC), dispatch_get_main_queue(), ^{
                KrRecStatus s;
                kr_rec_status(rec, &s);
                t0 = hostNow() - s.elapsed; // the recording's 0:00 on the host clock
            });
            dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)((0.5 + seconds) * NSEC_PER_SEC)), dispatch_get_main_queue(), ^{
                KrRecStatus s;
                char error[1024] = {0};
                // Share of one CPU core while recording (before the files are finished).
                info[@"cpu_percent"] = @(round((cpuSeconds() - cpu0) / (hostNow() - wall0) * 1000) / 10);
                int result = kr_rec_stop(rec, &s, error, sizeof error);
                info[@"status"] = statusJson(&s);
                if (result) info[@"error"] = @(error);
                if (flash) {
                    NSMutableArray *presses = [NSMutableArray array];
                    for (NSNumber *p in flash.presses) [presses addObject:@(p.doubleValue - t0)];
                    info[@"presses"] = presses; // seconds on the recording's timeline
                }
                writeJson([out stringByAppendingPathComponent:@"bench.json"], info);
                fprintf(stderr, "done: %lld frames, %lld new, %lld repeated\n", s.frames, s.frames_new, s.frames_repeated);
                exit(result ? 1 : 0);
            });
        };
        if (flash) {
            // The sync test starts with the user's first press (the window is then in front
            // and has the keyboard).
            fprintf(stderr, "waiting for the first press\n");
            flash.onFirstPress = begin;
        } else if (clap) {
            fprintf(stderr, "clap about once a second in view of the camera; starting in 3 s\n");
            dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 3 * NSEC_PER_SEC), dispatch_get_main_queue(), begin);
        } else {
            // Starts once the pattern window is on screen.
            dispatch_after(dispatch_time(DISPATCH_TIME_NOW, NSEC_PER_SEC / 2), dispatch_get_main_queue(), begin);
        }
        [app run];
    }
    return 0;
}
