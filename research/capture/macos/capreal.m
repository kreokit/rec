// capreal: M1 capture research with REAL devices on macOS 12.3+ (docs/capture-research.md).
// Only run with the user's permission: it records the real screen, camera and microphone.
//
// The same recording path as capbench (VideoToolbox H.264 -> fragmented MP4, one file per
// source, all on the host clock), fed by:
//   --screen sck   ScreenCaptureKit (macOS 12.3+), main display
//   --screen avf   AVCaptureScreenInput (AVFoundation, the pre-12.3 way)
//   --screen cgd   CGDisplayStream (CoreGraphics; what OBS uses for displays on macOS 12)
//   --camera       the default camera at 1280x720, frame rate fixed to --fps
//   --mic          the default microphone, converted to 48 kHz mono float
//
// To measure the real screen exactly, capreal shows a small window with the frame counter
// and sync marker of capbench, driven by the host clock; analyze.py reads it back from the
// recording (pattern.json gives its position). --beep plays a 5 ms click through the
// speakers on every marker, so the mic recording measures A/V sync end to end.
// --hide-pattern asks the system to keep that window out of the capture (sharingType none,
// and for ScreenCaptureKit also the content filter): if it works, the counter is missing
// from the recording. That is how KreoKit's own windows would be excluded (M5).
//
// Built as an app bundle (make-capreal-app.sh) so macOS asks for permission for capreal
// itself rather than for the editor or terminal that starts it.

#include "common.h"
#import <AppKit/AppKit.h>
#import <ScreenCaptureKit/ScreenCaptureKit.h>

static CMTime gT0;
static int gFps = 30;
static const CGFloat kPatternCells = 34;

// ---------------------------------------------------------------------------------------
// The on-screen pattern: frame counter + marker strip, from the host clock.

@interface PatternView : NSView
@end
@implementation PatternView
- (BOOL)isFlipped { return YES; }
- (void)drawRect:(NSRect)dirty {
    CGFloat cell = self.bounds.size.width / kPatternCells;
    double since = CMTimeGetSeconds(CMTimeSubtract(hostNow(), gT0));
    int64_t n = since < 0 ? 0 : (int64_t)floor(since * gFps);
    [[NSColor colorWithWhite:0.5 alpha:1] setFill];
    NSRectFill(NSMakeRect(0, 0, self.bounds.size.width, cell * 2));
    for (int b = 0; b < 32; b++) {
        [((n >> (31 - b)) & 1) ? NSColor.whiteColor : NSColor.blackColor setFill];
        NSRectFill(NSMakeRect(cell / 2 + b * cell, cell / 2, cell, cell));
    }
    [((n % gFps) == 0 && since >= 0) ? NSColor.whiteColor : NSColor.blackColor setFill];
    NSRectFill(NSMakeRect(0, cell * 2, self.bounds.size.width, cell * 2));
}
@end

// ---------------------------------------------------------------------------------------

@interface Stats : NSObject
@property(nonatomic) NSMutableArray<NSNumber *> *pts;     // seconds since t0
@property(nonatomic) NSMutableArray<NSNumber *> *latency; // ms from capture time to arrival
@property(nonatomic) int64_t captureDrops, idle, samples;
@property(nonatomic) double firstPts, lastEnd;
@end
@implementation Stats
- (instancetype)init {
    if ((self = [super init])) {
        _pts = [NSMutableArray array];
        _latency = [NSMutableArray array];
        _firstPts = -1;
    }
    return self;
}
@end

static NSString *statsJson(Stats *s) {
    NSMutableArray *gaps = [NSMutableArray array];
    for (NSUInteger i = 1; i < s.pts.count; i++) [gaps addObject:@(1000 * (s.pts[i].doubleValue - s.pts[i - 1].doubleValue))];
    NSArray *g = [gaps sortedArrayUsingSelector:@selector(compare:)];
    NSArray *l = [s.latency sortedArrayUsingSelector:@selector(compare:)];
    double (^q)(NSArray *, double) = ^double(NSArray *a, double p) {
        return a.count ? [a[(NSUInteger)round((a.count - 1) * p)] doubleValue] : 0;
    };
    int over50 = 0;
    for (NSNumber *x in gaps) over50 += x.doubleValue > 50;
    return [NSString stringWithFormat:@"\"buffers\": %lu, \"interval_ms_p50\": %.2f, \"interval_ms_p99\": %.2f, \"interval_ms_max\": %.2f, \"gaps_over_50ms\": %d, \"latency_ms_p50\": %.2f, \"latency_ms_max\": %.2f, \"capture_drops\": %lld, \"idle_frames\": %lld",
                                      (unsigned long)s.pts.count, q(g, 0.5), q(g, 0.99), q(g, 1.0), over50, q(l, 0.5), q(l, 1.0), s.captureDrops, s.idle];
}

// Timestamps from an AVCaptureSession are on the session's clock; the files use host time.
static CMSampleBufferRef retimedToHost(CMSampleBufferRef sb, CMClockRef clock) {
    CMItemCount count = 0;
    CMSampleBufferGetSampleTimingInfoArray(sb, 0, NULL, &count);
    CMSampleTimingInfo *timing = calloc((size_t)MAX(count, 1), sizeof *timing);
    CMSampleBufferGetSampleTimingInfoArray(sb, count, timing, &count);
    CMClockRef host = CMClockGetHostTimeClock();
    for (CMItemCount i = 0; i < count; i++) {
        timing[i].presentationTimeStamp = CMSyncConvertTime(timing[i].presentationTimeStamp, clock, host);
        timing[i].decodeTimeStamp = kCMTimeInvalid;
    }
    CMSampleBufferRef copy = NULL;
    CMSampleBufferCreateCopyWithNewTiming(NULL, sb, count, timing, &copy);
    free(timing);
    return copy;
}

@interface Recorder : NSObject <AVCaptureVideoDataOutputSampleBufferDelegate, AVCaptureAudioDataOutputSampleBufferDelegate, SCStreamOutput, SCStreamDelegate>
@property(nonatomic) NSMutableDictionary<NSString *, Track *> *tracks;
@property(nonatomic) NSMutableDictionary<NSString *, Stats *> *stats;
@property(nonatomic) AVCaptureSession *session;
@property(nonatomic) AVCaptureSession *screenSession;
@property(nonatomic) SCStream *scStream API_AVAILABLE(macos(12.3));
@property(nonatomic) AVCaptureConnection *cameraConnection, *screenConnection;
@property(nonatomic) CGDisplayStreamRef displayStream;
@property(nonatomic) volatile BOOL stopping;
@property(nonatomic) NSString *error;
@end

@implementation Recorder

- (void)append:(CMSampleBufferRef)sb to:(NSString *)name {
    Track *t = self.tracks[name];
    Stats *s = self.stats[name];
    CMTime pts = CMSampleBufferGetPresentationTimeStamp(sb);
    if (self.stopping || CMTimeCompare(pts, gT0) < 0) return;
    double sec = CMTimeGetSeconds(CMTimeSubtract(pts, gT0));
    [s.pts addObject:@(sec)];
    [s.latency addObject:@(1000 * CMTimeGetSeconds(CMTimeSubtract(hostNow(), pts)))];
    if (s.firstPts < 0) s.firstPts = sec;
    CMItemCount n = CMSampleBufferGetNumSamples(sb);
    if ([name isEqual:@"mic"]) {
        s.samples += n;
        s.lastEnd = sec + CMTimeGetSeconds(CMSampleBufferGetDuration(sb));
    }
    CFRetain(sb);
    dispatch_async(t.queue, ^{
        if ([name isEqual:@"mic"]) appendTimed(t, ^BOOL { return [t.input appendSampleBuffer:sb]; });
        else {
            CVPixelBufferRef pb = CMSampleBufferGetImageBuffer(sb);
            appendTimed(t, ^BOOL { return [t.adaptor appendPixelBuffer:pb withPresentationTime:pts]; });
        }
        CFRelease(sb);
    });
}

// AVFoundation camera, mic and AVCaptureScreenInput.
- (void)captureOutput:(AVCaptureOutput *)output didOutputSampleBuffer:(CMSampleBufferRef)sb fromConnection:(AVCaptureConnection *)connection {
    AVCaptureSession *owner = connection == self.screenConnection ? self.screenSession : self.session;
    CMClockRef clock = NULL;
    if (@available(macOS 12.3, *)) clock = owner.synchronizationClock;
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
    if (!clock) clock = owner.masterClock;
#pragma clang diagnostic pop
    CMSampleBufferRef host = retimedToHost(sb, clock);
    if (!host) return;
    NSString *name = [output isKindOfClass:AVCaptureAudioDataOutput.class] ? @"mic" : connection == self.cameraConnection ? @"camera" : @"screen";
    [self append:host to:name];
    CFRelease(host);
}

- (void)captureOutput:(AVCaptureOutput *)output didDropSampleBuffer:(CMSampleBufferRef)sb fromConnection:(AVCaptureConnection *)connection {
    NSString *name = connection == self.cameraConnection ? @"camera" : connection == self.screenConnection ? @"screen" : @"mic";
    self.stats[name].captureDrops++;
}

// ScreenCaptureKit: timestamps are already host time. Frames whose status isn't
// "complete" (idle: nothing changed) carry no new picture.
- (void)stream:(SCStream *)stream didOutputSampleBuffer:(CMSampleBufferRef)sb ofType:(SCStreamOutputType)type API_AVAILABLE(macos(12.3)) {
    if (type != SCStreamOutputTypeScreen) return;
    CFArrayRef attachments = CMSampleBufferGetSampleAttachmentsArray(sb, false);
    if (!attachments || CFArrayGetCount(attachments) == 0) return;
    NSDictionary *info = (__bridge NSDictionary *)CFArrayGetValueAtIndex(attachments, 0);
    NSNumber *status = info[SCStreamFrameInfoStatus];
    if (status.integerValue != SCFrameStatusComplete || !CMSampleBufferGetImageBuffer(sb)) {
        self.stats[@"screen"].idle++;
        return;
    }
    [self append:sb to:@"screen"];
}

- (void)stream:(SCStream *)stream didStopWithError:(NSError *)error API_AVAILABLE(macos(12.3)) {
    self.error = [NSString stringWithFormat:@"SCStream stopped: %@", error.localizedDescription];
}

@end

// ---------------------------------------------------------------------------------------

// The format and frame-rate range for w x h at fps. Use the range's own frame duration:
// the MacBook's FaceTime HD camera offers "30.00" as 1000000/30000030 s and throws on an
// exact 1/30 s (measured 2026-10-26).
static AVCaptureDeviceFormat *formatFor(AVCaptureDevice *d, int w, int h, int fps, AVFrameRateRange **range) {
    for (AVCaptureDeviceFormat *f in d.formats) {
        CMVideoDimensions dim = CMVideoFormatDescriptionGetDimensions(f.formatDescription);
        if (dim.width != w || dim.height != h) continue;
        for (AVFrameRateRange *r in f.videoSupportedFrameRateRanges)
            if (r.maxFrameRate >= fps - 0.01 && r.minFrameRate <= fps + 0.01) {
                *range = r;
                return f;
            }
    }
    return nil;
}

static const char *authName(AVAuthorizationStatus s) {
    switch (s) {
    case AVAuthorizationStatusAuthorized: return "allowed";
    case AVAuthorizationStatusDenied: return "denied";
    case AVAuthorizationStatusRestricted: return "restricted";
    default: return "not asked";
    }
}

// --check prints what capreal may record; --request first asks for camera and mic (waiting
// for the answers) and opens the screen-recording request (System Preferences on macOS 12).
static int permissions(BOOL request) {
    if (request) {
        for (AVMediaType type in @[ AVMediaTypeVideo, AVMediaTypeAudio ]) {
            if ([AVCaptureDevice authorizationStatusForMediaType:type] != AVAuthorizationStatusNotDetermined) continue;
            dispatch_semaphore_t answered = dispatch_semaphore_create(0);
            [AVCaptureDevice requestAccessForMediaType:type completionHandler:^(BOOL granted) { dispatch_semaphore_signal(answered); }];
            dispatch_semaphore_wait(answered, DISPATCH_TIME_FOREVER);
        }
        if (!CGPreflightScreenCaptureAccess()) CGRequestScreenCaptureAccess();
    }
    printf("{\"screen\": \"%s\", \"camera\": \"%s\", \"mic\": \"%s\"}\n", CGPreflightScreenCaptureAccess() ? "allowed" : "not allowed",
           authName([AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeVideo]), authName([AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeAudio]));
    fflush(stdout);
    return 0;
}

static NSString *jsonString(NSString *s) {
    NSData *d = [NSJSONSerialization dataWithJSONObject:@[ s ?: @"" ] options:0 error:nil];
    NSString *a = [[NSString alloc] initWithData:d encoding:NSUTF8StringEncoding];
    return [a substringWithRange:NSMakeRange(1, a.length - 2)];
}

int main(int argc, const char *argv[]) {
    @autoreleasepool {
        NSString *out = nil, *screenMode = @"none";
        double seconds = 10, fragment = 1.0;
        int sw = 1728, sh = 1080;
        BOOL camera = NO, mic = NO, beep = NO, pattern = YES, hidePattern = NO;
        for (int i = 1; i < argc; i++) {
            const char *a = argv[i];
            const char *v = i + 1 < argc ? argv[i + 1] : "";
            if (!strcmp(a, "--out")) out = @(v), i++;
            else if (!strcmp(a, "--seconds")) seconds = atof(v), i++;
            else if (!strcmp(a, "--screen")) screenMode = @(v), i++;
            else if (!strcmp(a, "--size")) parseSize(v, &sw, &sh), i++;
            else if (!strcmp(a, "--fps")) gFps = atoi(v), i++;
            else if (!strcmp(a, "--fragment")) fragment = atof(v), i++;
            else if (!strcmp(a, "--keyframe")) gKeyframeSeconds = atof(v), i++;
            else if (!strcmp(a, "--camera")) camera = YES;
            else if (!strcmp(a, "--mic")) mic = YES;
            else if (!strcmp(a, "--beep")) beep = YES;
            else if (!strcmp(a, "--no-pattern")) pattern = NO;
            else if (!strcmp(a, "--hide-pattern")) hidePattern = YES;
            else if (!strcmp(a, "--check")) return permissions(NO);
            else if (!strcmp(a, "--request")) return permissions(YES);
            else if (!strncmp(a, "-psn_", 5)) continue; // added by Launch Services
            else {
                fprintf(stderr, "unknown option %s\n", a);
                return 2;
            }
        }
        if (!out) {
            fprintf(stderr, "usage: capreal --out DIR [--seconds N] [--screen sck|avf|cgd|none] [--size WxH] [--fps N] [--camera] [--mic] [--beep] [--no-pattern] [--hide-pattern]\n");
            return 2;
        }
        [[NSFileManager defaultManager] createDirectoryAtPath:out withIntermediateDirectories:YES attributes:nil error:nil];
        NSApplication *app = [NSApplication sharedApplication];
        app.activationPolicy = NSApplicationActivationPolicyAccessory;

        Recorder *rec = [Recorder new];
        rec.tracks = [NSMutableDictionary dictionary];
        rec.stats = [NSMutableDictionary dictionary];
        NSMutableDictionary *report = [NSMutableDictionary dictionary];
        // Sources start 3 s early (camera exposure settles); files start at t0.
        gT0 = CMTimeAdd(hostNow(), CMTimeMake(3, 1));

        // The pattern window, top-left of the main screen's visible area.
        NSScreen *screen = NSScreen.mainScreen;
        CGFloat scale = sw / screen.frame.size.width; // capture pixels per point
        CGFloat cellPt = round(16 / scale * 4) / 4;
        NSWindow *window = nil;
        PatternView *view = nil;
        if (pattern && ![screenMode isEqual:@"none"]) {
            NSRect vis = screen.visibleFrame;
            NSRect frame = NSMakeRect(vis.origin.x + 40, NSMaxY(vis) - 40 - cellPt * 4, cellPt * kPatternCells, cellPt * 4);
            window = [[NSWindow alloc] initWithContentRect:frame styleMask:NSWindowStyleMaskBorderless backing:NSBackingStoreBuffered defer:NO];
            window.level = NSStatusWindowLevel;
            if (hidePattern) window.sharingType = NSWindowSharingNone;
            view = [[PatternView alloc] initWithFrame:NSMakeRect(0, 0, frame.size.width, frame.size.height)];
            window.contentView = view;
            [window orderFrontRegardless];
            [NSTimer scheduledTimerWithTimeInterval:1.0 / 120 repeats:YES block:^(NSTimer *t) { view.needsDisplay = YES; }];
            CGFloat top = NSMaxY(screen.frame) - NSMaxY(frame);
            NSDictionary *geom = @{
                @"cell" : @(cellPt * scale),
                @"x" : @((frame.origin.x - screen.frame.origin.x) * scale),
                @"y" : @(top * scale),
                @"marker_y" : @((top + cellPt * 3) * scale),
            };
            [[NSJSONSerialization dataWithJSONObject:geom options:0 error:nil] writeToFile:[out stringByAppendingString:@"/pattern.json"] atomically:YES];
        }

        void (^addVideoTrack)(NSString *, int, int, int) = ^(NSString *name, int w, int h, int bitrate) {
            Track *t = makeTrack(out, name, AVFileTypeMPEG4, fragment, AVMediaTypeVideo, h264Settings(w, h, gFps, bitrate));
            t.adaptor = [AVAssetWriterInputPixelBufferAdaptor assetWriterInputPixelBufferAdaptorWithAssetWriterInput:t.input sourcePixelBufferAttributes:nil];
            rec.tracks[name] = t;
            rec.stats[name] = [Stats new];
        };
        dispatch_queue_t captureQueue = dispatch_queue_create("capture", dispatch_queue_attr_make_with_qos_class(DISPATCH_QUEUE_SERIAL, QOS_CLASS_USER_INTERACTIVE, 0));
        NSDictionary *nv12 = @{(id)kCVPixelBufferPixelFormatTypeKey : @(kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange)};

        // Camera + mic: one AVCaptureSession.
        if (camera || mic) {
            rec.session = [AVCaptureSession new];
            [rec.session beginConfiguration];
            if (camera) {
                AVCaptureDevice *dev = [AVCaptureDevice defaultDeviceWithMediaType:AVMediaTypeVideo];
                NSError *e = nil;
                AVCaptureDeviceInput *input = dev ? [AVCaptureDeviceInput deviceInputWithDevice:dev error:&e] : nil;
                if (!input) {
                    fprintf(stderr, "camera: %s\n", e ? e.localizedDescription.UTF8String : "none found");
                    return 1;
                }
                [rec.session addInput:input];
                AVFrameRateRange *range = nil;
                AVCaptureDeviceFormat *fmt = formatFor(dev, 1280, 720, gFps, &range);
                if (fmt && [dev lockForConfiguration:&e]) {
                    dev.activeFormat = fmt;
                    // A fixed frame rate: otherwise the camera may halve it in low light. A
                    // range offering gFps exactly has min = max duration; else the fastest.
                    CMTime duration = range.maxFrameRate - range.minFrameRate < 0.01 ? range.minFrameDuration : CMTimeMake(1, gFps);
                    dev.activeVideoMinFrameDuration = duration;
                    dev.activeVideoMaxFrameDuration = duration;
                    [dev unlockForConfiguration];
                }
                CMVideoDimensions dim = CMVideoFormatDescriptionGetDimensions(dev.activeFormat.formatDescription);
                report[@"camera_device"] = [NSString stringWithFormat:@"%@ %dx%d (720p30 format found: %@)", dev.localizedName, dim.width, dim.height, fmt ? @"yes" : @"no"];
                AVCaptureVideoDataOutput *vo = [AVCaptureVideoDataOutput new];
                vo.videoSettings = nv12;
                vo.alwaysDiscardsLateVideoFrames = YES;
                [vo setSampleBufferDelegate:rec queue:captureQueue];
                [rec.session addOutput:vo];
                rec.cameraConnection = [vo connectionWithMediaType:AVMediaTypeVideo];
                addVideoTrack(@"camera", dim.width, dim.height, 3000000);
            }
            if (mic) {
                AVCaptureDevice *dev = [AVCaptureDevice defaultDeviceWithMediaType:AVMediaTypeAudio];
                NSError *e = nil;
                AVCaptureDeviceInput *input = dev ? [AVCaptureDeviceInput deviceInputWithDevice:dev error:&e] : nil;
                if (!input) {
                    fprintf(stderr, "mic: %s\n", e ? e.localizedDescription.UTF8String : "none found");
                    return 1;
                }
                [rec.session addInput:input];
                report[@"mic_device"] = dev.localizedName;
                AVCaptureAudioDataOutput *ao = [AVCaptureAudioDataOutput new];
                ao.audioSettings = @{
                    AVFormatIDKey : @(kAudioFormatLinearPCM), AVSampleRateKey : @48000, AVNumberOfChannelsKey : @1,
                    AVLinearPCMBitDepthKey : @32, AVLinearPCMIsFloatKey : @YES, AVLinearPCMIsNonInterleaved : @NO,
                };
                [ao setSampleBufferDelegate:rec queue:dispatch_queue_create("mic", DISPATCH_QUEUE_SERIAL)];
                [rec.session addOutput:ao];
                Track *t = makeTrack(out, @"mic", AVFileTypeMPEG4, fragment, AVMediaTypeAudio,
                                     @{AVFormatIDKey : @(kAudioFormatMPEG4AAC), AVSampleRateKey : @48000, AVNumberOfChannelsKey : @1, AVEncoderBitRateKey : @128000});
                rec.tracks[@"mic"] = t;
                rec.stats[@"mic"] = [Stats new];
            }
            [rec.session commitConfiguration];
        }

        CGDirectDisplayID displayID = [screen.deviceDescription[@"NSScreenNumber"] unsignedIntValue];
        report[@"display"] = [NSString stringWithFormat:@"%zux%zu px, %.0fx%.0f pt", CGDisplayPixelsWide(displayID) * (size_t)screen.backingScaleFactor,
                                                        CGDisplayPixelsHigh(displayID) * (size_t)screen.backingScaleFactor, screen.frame.size.width, screen.frame.size.height];
        if ([screenMode isEqual:@"avf"]) {
            addVideoTrack(@"screen", sw, sh, 8000000);
            rec.screenSession = [AVCaptureSession new];
            AVCaptureScreenInput *input = [[AVCaptureScreenInput alloc] initWithDisplayID:displayID];
            input.minFrameDuration = CMTimeMake(1, gFps);
            input.scaleFactor = sw / (screen.frame.size.width * screen.backingScaleFactor);
            input.capturesCursor = YES;
            [rec.screenSession addInput:input];
            AVCaptureVideoDataOutput *vo = [AVCaptureVideoDataOutput new];
            vo.videoSettings = nv12;
            vo.alwaysDiscardsLateVideoFrames = YES;
            [vo setSampleBufferDelegate:rec queue:captureQueue];
            [rec.screenSession addOutput:vo];
            rec.screenConnection = [vo connectionWithMediaType:AVMediaTypeVideo];
        } else if ([screenMode isEqual:@"cgd"]) {
            addVideoTrack(@"screen", sw, sh, 8000000);
            NSDictionary *props = @{
                (__bridge NSString *)kCGDisplayStreamMinimumFrameTime : @(1.0 / gFps),
                (__bridge NSString *)kCGDisplayStreamShowCursor : @YES,
                (__bridge NSString *)kCGDisplayStreamYCbCrMatrix : (__bridge NSString *)kCGDisplayStreamYCbCrMatrix_ITU_R_709_2,
                (__bridge NSString *)kCGDisplayStreamQueueDepth : @8,
            };
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
            rec.displayStream = CGDisplayStreamCreateWithDispatchQueue(
                displayID, (size_t)sw, (size_t)sh, kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange, (__bridge CFDictionaryRef)props, captureQueue,
                ^(CGDisplayStreamFrameStatus status, uint64_t displayTime, IOSurfaceRef surface, CGDisplayStreamUpdateRef update) {
                    if (status != kCGDisplayStreamFrameStatusFrameComplete || !surface) {
                        if (status == kCGDisplayStreamFrameStatusFrameIdle) rec.stats[@"screen"].idle++;
                        return;
                    }
                    // displayTime is mach_absolute_time: the host clock.
                    CVPixelBufferRef pb = NULL;
                    CVPixelBufferCreateWithIOSurface(NULL, surface, NULL, &pb);
                    CMVideoFormatDescriptionRef desc = NULL;
                    CMVideoFormatDescriptionCreateForImageBuffer(NULL, pb, &desc);
                    CMSampleTimingInfo timing = {kCMTimeInvalid, CMClockMakeHostTimeFromSystemUnits(displayTime), kCMTimeInvalid};
                    CMSampleBufferRef sb = NULL;
                    CMSampleBufferCreateReadyWithImageBuffer(NULL, pb, desc, &timing, &sb);
                    if (sb) [rec append:sb to:@"screen"], CFRelease(sb);
                    if (desc) CFRelease(desc);
                    CVPixelBufferRelease(pb);
                });
#pragma clang diagnostic pop
            if (!rec.displayStream) {
                fprintf(stderr, "CGDisplayStream: could not create the stream. Allow Screen Recording for capreal in System Preferences > Security & Privacy, then run again.\n");
                return 1;
            }
        } else if ([screenMode isEqual:@"sck"]) {
            if (!NSClassFromString(@"SCStream")) {
                fprintf(stderr, "ScreenCaptureKit needs macOS 12.3 or newer\n");
                return 1;
            }
            if (@available(macOS 12.3, *)) {
                addVideoTrack(@"screen", sw, sh, 8000000);
                dispatch_semaphore_t got = dispatch_semaphore_create(0);
                __block SCShareableContent *content = nil;
                __block NSError *err = nil;
                [SCShareableContent getShareableContentWithCompletionHandler:^(SCShareableContent *c, NSError *e) {
                    content = c, err = e;
                    dispatch_semaphore_signal(got);
                }];
                dispatch_semaphore_wait(got, DISPATCH_TIME_FOREVER);
                SCDisplay *display = nil;
                for (SCDisplay *d in content.displays)
                    if (d.displayID == displayID) display = d;
                if (!display) {
                    fprintf(stderr, "ScreenCaptureKit: no display (%s). Allow Screen Recording for capreal in System Preferences > Security & Privacy, then run again.\n",
                            err ? err.localizedDescription.UTF8String : "permission?");
                    return 1;
                }
                NSMutableArray<SCWindow *> *excluded = [NSMutableArray array];
                if (hidePattern)
                    for (SCWindow *w in content.windows)
                        if (w.windowID == (CGWindowID)window.windowNumber) [excluded addObject:w];
                report[@"excluded_windows"] = [NSString stringWithFormat:@"%lu", (unsigned long)excluded.count];
                SCContentFilter *filter = [[SCContentFilter alloc] initWithDisplay:display excludingWindows:excluded];
                SCStreamConfiguration *cfg = [SCStreamConfiguration new];
                cfg.width = (size_t)sw;
                cfg.height = (size_t)sh;
                cfg.minimumFrameInterval = CMTimeMake(1, gFps);
                cfg.pixelFormat = kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange;
                cfg.colorMatrix = kCGDisplayStreamYCbCrMatrix_ITU_R_709_2;
                cfg.queueDepth = 8;
                cfg.showsCursor = YES;
                rec.scStream = [[SCStream alloc] initWithFilter:filter configuration:cfg delegate:rec];
                NSError *e = nil;
                if (![rec.scStream addStreamOutput:rec type:SCStreamOutputTypeScreen sampleHandlerQueue:captureQueue error:&e]) {
                    fprintf(stderr, "SCStream output: %s\n", e.localizedDescription.UTF8String);
                    return 1;
                }
            }
        }

        // Clicks through the speakers on every marker (host-clock exact, like the pattern).
        AVAudioEngine *engine = nil;
        if (beep) {
            engine = [AVAudioEngine new];
            AVAudioFormat *fmt = [engine.outputNode outputFormatForBus:0];
            double rate = fmt.sampleRate;
            static mach_timebase_info_data_t tb;
            mach_timebase_info(&tb);
            AVAudioSourceNode *src = [[AVAudioSourceNode alloc] initWithRenderBlock:^OSStatus(BOOL *silence, const AudioTimeStamp *ts, AVAudioFrameCount frames, AudioBufferList *abl) {
                double host0 = (double)ts->mHostTime * tb.numer / tb.denom / 1e9;
                double t0s = CMTimeGetSeconds(gT0);
                for (AVAudioFrameCount i = 0; i < frames; i++) {
                    double since = host0 + i / rate - t0s;
                    float v = (since >= 0 && fmod(since, 1.0) < 0.005) ? 0.5f : 0.0f;
                    for (UInt32 b = 0; b < abl->mNumberBuffers; b++) ((float *)abl->mBuffers[b].mData)[i] = v;
                }
                return noErr;
            }];
            [engine attachNode:src];
            [engine connect:src to:engine.mainMixerNode format:[[AVAudioFormat alloc] initStandardFormatWithSampleRate:rate channels:1]];
            NSError *e = nil;
            if (![engine startAndReturnError:&e]) fprintf(stderr, "beep: %s\n", e.localizedDescription.UTF8String);
        }

        for (Track *t in rec.tracks.allValues) startTrack(t, gT0);
        [rec.session startRunning];
        [rec.screenSession startRunning];
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
        if (rec.displayStream) CGDisplayStreamStart(rec.displayStream);
#pragma clang diagnostic pop
        if (@available(macOS 12.3, *)) {
            if (rec.scStream)
                [rec.scStream startCaptureWithCompletionHandler:^(NSError *e) {
                    if (e) rec.error = [NSString stringWithFormat:@"SCStream start: %@", e.localizedDescription];
                }];
        }

        __block uint64_t idle1 = 0, idle2 = 0, tot1 = 0;
        __block double proc0 = 0;
        dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(CMTimeGetSeconds(CMTimeSubtract(gT0, hostNow())) * NSEC_PER_SEC)), dispatch_get_main_queue(), ^{
            tot1 = cpuTicks(&idle1);
            proc0 = processCpuSeconds();
            fprintf(stderr, "recording %.0f s into %s\n", seconds, out.UTF8String);
        });
        dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)((CMTimeGetSeconds(CMTimeSubtract(gT0, hostNow())) + seconds) * NSEC_PER_SEC)), dispatch_get_main_queue(), ^{
            uint64_t tot2 = cpuTicks(&idle2);
            double proc = processCpuSeconds() - proc0;
            rec.stopping = YES;
            [rec.session stopRunning];
            [rec.screenSession stopRunning];
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
            if (rec.displayStream) CGDisplayStreamStop(rec.displayStream);
#pragma clang diagnostic pop
            if (@available(macOS 12.3, *)) {
                if (rec.scStream) {
                    dispatch_semaphore_t stopped = dispatch_semaphore_create(0);
                    [rec.scStream stopCaptureWithCompletionHandler:^(NSError *e) { dispatch_semaphore_signal(stopped); }];
                    dispatch_semaphore_wait(stopped, dispatch_time(DISPATCH_TIME_NOW, 3 * NSEC_PER_SEC));
                }
            }
            [engine stop];
            dispatch_sync(captureQueue, ^{});
            dispatch_group_t done = dispatch_group_create();
            for (Track *t in rec.tracks.allValues) {
                dispatch_group_enter(done);
                dispatch_async(t.queue, ^{
                    [t.input markAsFinished];
                    [t.writer finishWritingWithCompletionHandler:^{ dispatch_group_leave(done); }];
                });
            }
            dispatch_group_wait(done, DISPATCH_TIME_FOREVER);

            NSMutableString *json = [NSMutableString stringWithString:@"{\n"];
            [json appendFormat:@"  \"screen_mode\": \"%@\", \"seconds\": %.1f, \"logical_cpus\": %ld,\n", screenMode, seconds, (long)NSProcessInfo.processInfo.activeProcessorCount];
            for (NSString *k in report) [json appendFormat:@"  \"%@\": %@,\n", k, jsonString(report[k])];
            [json appendFormat:@"  \"process_cpu_percent_of_one_core\": %.1f,\n", 100 * proc / seconds];
            [json appendFormat:@"  \"system_busy_percent_all_cores\": %.1f,\n", 100 * (1.0 - (double)(idle2 - idle1) / (double)(tot2 - tot1))];
            if (rec.error) [json appendFormat:@"  \"error\": %@,\n", jsonString(rec.error)];
            [json appendString:@"  \"tracks\": [\n"];
            NSArray *names = [rec.tracks.allKeys sortedArrayUsingSelector:@selector(compare:)];
            for (NSUInteger i = 0; i < names.count; i++) {
                Track *t = rec.tracks[names[i]];
                Stats *s = rec.stats[names[i]];
                NSString *extra = @"";
                if ([names[i] isEqual:@"mic"] && s.lastEnd > s.firstPts)
                    // Samples received vs the host-clock span they cover: the mic clock's drift.
                    extra = [NSString stringWithFormat:@", \"samples\": %lld, \"sample_clock_vs_host_ppm\": %.1f", s.samples, 1e6 * (s.samples / 48000.0 / (s.lastEnd - s.firstPts) - 1)];
                [json appendFormat:@"    {\"name\": \"%@\", \"writer_status\": %ld, \"appended\": %lld, \"dropped_not_ready\": %lld, \"append_failed\": %lld, %@%@%@}%@\n", names[i],
                                   (long)t.writer.status, t.appended, t.droppedNotReady, t.appendFailed, statsJson(s), extra,
                                   t.writer.error ? [NSString stringWithFormat:@", \"error\": %@", jsonString(t.writer.error.localizedDescription)] : @"",
                                   i + 1 < names.count ? @"," : @""];
            }
            [json appendString:@"  ]\n}\n"];
            [json writeToFile:[out stringByAppendingString:@"/report.json"] atomically:YES encoding:NSUTF8StringEncoding error:nil];
            printf("%s", json.UTF8String);
            fflush(stdout);
            exit(0);
        });
        [app run];
    }
    return 0;
}
