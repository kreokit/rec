// capbench: M1 capture research prototype for macOS 12+ (see docs/capture-research.md).
//
// Records synthetic sources through the recording path we plan for the app:
// pixel buffers -> VideoToolbox H.264 (AVAssetWriter, real time) -> fragmented MP4, one
// file per source, every source on the host clock. It measures what the app must
// guarantee: CPU use, dropped frames, timing jitter and what survives a crash (kill -9).
//
// Synthetic video: a static detailed background, a moving band (so the encoder has work)
// and the frame number drawn as 32 black/white squares in the top-left corner, so
// `analyze.py` can find gaps. Every 30th frame also gets a white strip at the bottom: the
// sync marker. Synthetic mic: silence with a 5 ms click at every whole second from the
// start, the same instants as the markers.
//
//   clang -fobjc-arc -O2 -mmacosx-version-min=12.0 capbench.m -o capbench \
//     -framework Foundation -framework AVFoundation -framework CoreMedia \
//     -framework CoreVideo -framework VideoToolbox
//   ./capbench --out DIR --seconds 600 --screen 1728x1080 --camera 1280x720 --mic

#include "common.h"

// ---------------------------------------------------------------------------------------
// Synthetic video.

typedef struct {
    int w, h, fps;
    BOOL nv12;
} VideoFormat;

static void drawBackground(CVPixelBufferRef pb, VideoFormat f) {
    // A detailed, mostly static picture like a desktop: fine checker + gradients.
    if (f.nv12) {
        uint8_t *y = CVPixelBufferGetBaseAddressOfPlane(pb, 0);
        size_t ys = CVPixelBufferGetBytesPerRowOfPlane(pb, 0);
        for (int r = 0; r < f.h; r++)
            for (int c = 0; c < f.w; c++) y[r * ys + c] = (uint8_t)(((r / 8 + c / 8) & 1) ? 60 + c % 128 : 180 - r % 100);
        uint8_t *uv = CVPixelBufferGetBaseAddressOfPlane(pb, 1);
        size_t uvs = CVPixelBufferGetBytesPerRowOfPlane(pb, 1);
        for (int r = 0; r < f.h / 2; r++)
            for (int c = 0; c < f.w / 2; c++) {
                uv[r * uvs + 2 * c] = (uint8_t)(100 + c % 56);
                uv[r * uvs + 2 * c + 1] = (uint8_t)(150 - r % 44);
            }
    } else {
        uint8_t *p = CVPixelBufferGetBaseAddress(pb);
        size_t s = CVPixelBufferGetBytesPerRow(pb);
        for (int r = 0; r < f.h; r++)
            for (int c = 0; c < f.w; c++) {
                uint8_t v = (uint8_t)(((r / 8 + c / 8) & 1) ? 60 + c % 128 : 180 - r % 100);
                uint8_t *px = p + r * s + 4 * c;
                px[0] = v; px[1] = (uint8_t)(255 - v); px[2] = (uint8_t)(c % 256); px[3] = 255;
            }
    }
}

// Sets a luma rectangle (NV12) or a grey rectangle (BGRA).
static void fillRect(CVPixelBufferRef pb, VideoFormat f, int x, int y, int w, int h, uint8_t v) {
    if (f.nv12) {
        uint8_t *base = CVPixelBufferGetBaseAddressOfPlane(pb, 0);
        size_t s = CVPixelBufferGetBytesPerRowOfPlane(pb, 0);
        for (int r = y; r < y + h && r < f.h; r++) memset(base + r * s + x, v, (size_t)MIN(w, f.w - x));
        uint8_t *uv = CVPixelBufferGetBaseAddressOfPlane(pb, 1);
        size_t uvs = CVPixelBufferGetBytesPerRowOfPlane(pb, 1);
        for (int r = y / 2; r < (y + h) / 2 && r < f.h / 2; r++) memset(uv + r * uvs + (x & ~1), 128, (size_t)MIN(w, f.w - x));
    } else {
        uint8_t *base = CVPixelBufferGetBaseAddress(pb);
        size_t s = CVPixelBufferGetBytesPerRow(pb);
        for (int r = y; r < y + h && r < f.h; r++) memset(base + r * s + 4 * x, v, (size_t)4 * MIN(w, f.w - x));
    }
}

static const int kCell = 16; // counter square size in pixels

static void drawFrame(CVPixelBufferRef pb, VideoFormat f, int64_t n) {
    // Pool buffers keep their pixels between uses; draw the background once per buffer.
    CFTypeRef seen = CVBufferCopyAttachment(pb, CFSTR("kkInit"), NULL);
    if (seen) CFRelease(seen);
    else {
        drawBackground(pb, f);
        CVBufferSetAttachment(pb, CFSTR("kkInit"), kCFBooleanTrue, kCVAttachmentMode_ShouldNotPropagate);
    }
    // The moving band: 1/8 of the height, sweeping down over 4 s. Old bands left in a
    // recycled buffer don't matter; the picture changes every frame either way.
    int band = f.h / 8;
    int y = (int)((n * band / 15) % (f.h - band));
    fillRect(pb, f, 0, y, f.w, band / 4, (uint8_t)(n * 7));
    // Frame counter: 32 squares, bit 31 first, white = 1, on a mid-grey row.
    fillRect(pb, f, 0, 0, kCell * 34, kCell * 2, 128);
    for (int b = 0; b < 32; b++)
        fillRect(pb, f, kCell / 2 + b * kCell, kCell / 2, kCell, kCell, ((n >> (31 - b)) & 1) ? 235 : 16);
    // Sync marker on every whole second; otherwise the strip is black.
    fillRect(pb, f, 0, f.h - kCell * 2, f.w, kCell * 2, (n % f.fps) == 0 ? 235 : 16);
}

static CVPixelBufferPoolRef makePool(VideoFormat f) {
    NSDictionary *attrs = @{
        (id)kCVPixelBufferPixelFormatTypeKey : @(f.nv12 ? kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange : kCVPixelFormatType_32BGRA),
        (id)kCVPixelBufferWidthKey : @(f.w),
        (id)kCVPixelBufferHeightKey : @(f.h),
        (id)kCVPixelBufferIOSurfacePropertiesKey : @{}, // like capture buffers: zero-copy to the encoder
    };
    CVPixelBufferPoolRef pool = NULL;
    NSDictionary *poolAttrs = @{(id)kCVPixelBufferPoolMinimumBufferCountKey : @8};
    CVPixelBufferPoolCreate(NULL, (__bridge CFDictionaryRef)poolAttrs, (__bridge CFDictionaryRef)attrs, &pool);
    return pool;
}

// ---------------------------------------------------------------------------------------

int main(int argc, const char *argv[]) {
    @autoreleasepool {
        NSString *out = nil;
        double seconds = 10, fragment = 1.0;
        int sw = 1728, sh = 1080, cw = 0, ch = 0, fps = 30;
        int screenBitrate = 8000000, cameraBitrate = 3000000;
        BOOL mic = NO, nv12 = YES;
        NSString *fileType = AVFileTypeMPEG4;
        for (int i = 1; i < argc; i++) {
            const char *a = argv[i];
            const char *v = i + 1 < argc ? argv[i + 1] : "";
            if (!strcmp(a, "--out")) out = @(v), i++;
            else if (!strcmp(a, "--seconds")) seconds = atof(v), i++;
            else if (!strcmp(a, "--screen")) parseSize(v, &sw, &sh), i++;
            else if (!strcmp(a, "--camera")) parseSize(v, &cw, &ch), i++;
            else if (!strcmp(a, "--fps")) fps = atoi(v), i++;
            else if (!strcmp(a, "--fragment")) fragment = atof(v), i++;
            else if (!strcmp(a, "--keyframe")) gKeyframeSeconds = atof(v), i++;
            else if (!strcmp(a, "--screen-bitrate")) screenBitrate = atoi(v), i++;
            else if (!strcmp(a, "--mic")) mic = YES;
            else if (!strcmp(a, "--bgra")) nv12 = NO;
            else if (!strcmp(a, "--mov")) fileType = AVFileTypeQuickTimeMovie;
            else {
                fprintf(stderr, "unknown option %s\n", a);
                return 2;
            }
        }
        if (!out) {
            fprintf(stderr, "usage: capbench --out DIR [--seconds N] [--screen WxH] [--camera WxH] [--fps N] [--mic] [--bgra] [--mov] [--fragment S] [--keyframe S]\n");
            return 2;
        }
        [[NSFileManager defaultManager] createDirectoryAtPath:out withIntermediateDirectories:YES attributes:nil error:nil];

        // Baseline system CPU for 3 s before anything starts.
        uint64_t idle0, idle1, idle2;
        uint64_t tot0 = cpuTicks(&idle0);
        [NSThread sleepForTimeInterval:3.0];
        uint64_t tot1 = cpuTicks(&idle1);
        double baselineBusy = 1.0 - (double)(idle1 - idle0) / (double)(tot1 - tot0);

        int progressFd = open([out stringByAppendingString:@"/progress.txt"].UTF8String, O_WRONLY | O_CREAT | O_TRUNC, 0644);
        NSMutableArray<Track *> *tracks = [NSMutableArray array];
        NSMutableArray *timers = [NSMutableArray array];
        __block BOOL stopping = NO;
        // Every source starts on the same host-clock instant, like a real recording.
        CMTime t0 = CMTimeAdd(hostNow(), CMTimeMake(2, 1)); // after setup
        dispatch_queue_t timerQueue = dispatch_queue_create("ticks", dispatch_queue_attr_make_with_qos_class(DISPATCH_QUEUE_SERIAL, QOS_CLASS_USER_INTERACTIVE, 0));
        NSMutableDictionary *jitter = [NSMutableDictionary dictionary]; // name -> max lateness ms

        void (^addVideo)(NSString *, VideoFormat, int) = ^(NSString *name, VideoFormat f, int bitrate) {
            Track *t = makeTrack(out, name, fileType, fragment, AVMediaTypeVideo, h264Settings(f.w, f.h, f.fps, bitrate));
            t.adaptor = [AVAssetWriterInputPixelBufferAdaptor assetWriterInputPixelBufferAdaptorWithAssetWriterInput:t.input sourcePixelBufferAttributes:nil];
            [tracks addObject:t];
            CVPixelBufferPoolRef pool = makePool(f);
            // Draw the background into the pool's buffers now, not while recording.
            CVPixelBufferRef warm[8];
            for (int i = 0; i < 8; i++) {
                CVPixelBufferPoolCreatePixelBuffer(NULL, pool, &warm[i]);
                CVPixelBufferLockBaseAddress(warm[i], 0);
                drawFrame(warm[i], f, 0);
                CVPixelBufferUnlockBaseAddress(warm[i], 0);
            }
            for (int i = 0; i < 8; i++) CVPixelBufferRelease(warm[i]);
            __block int64_t n = 0;
            __block double maxLate = 0;
            dispatch_source_t timer = dispatch_source_create(DISPATCH_SOURCE_TYPE_TIMER, 0, 0, timerQueue);
            dispatch_source_set_timer(timer, dispatch_time(DISPATCH_TIME_NOW, 200 * NSEC_PER_MSEC), NSEC_PER_SEC / f.fps, 0);
            dispatch_source_set_event_handler(timer, ^{
                if (stopping) return;
                // Like a capture callback: emit every frame that is due by now, each with its
                // exact presentation time, and count how late we were.
                CMTime now = hostNow();
                while (true) {
                    CMTime pts = CMTimeAdd(t0, CMTimeMake(n, f.fps));
                    if (CMTimeCompare(pts, now) > 0) break;
                    double late = CMTimeGetSeconds(CMTimeSubtract(now, pts)) * 1000;
                    if (late > maxLate) maxLate = late, jitter[name] = @(late);
                    int64_t frame = n++;
                    // Crash tests: the newest frame handed to the writer, readable after kill -9.
                    if (progressFd >= 0 && [name isEqual:@"screen"]) {
                        char line[32];
                        int len = snprintf(line, sizeof line, "%-20lld\n", frame);
                        pwrite(progressFd, line, (size_t)len, 0);
                    }
                    CVPixelBufferRef pb = NULL;
                    CVPixelBufferPoolCreatePixelBuffer(NULL, pool, &pb);
                    CVPixelBufferLockBaseAddress(pb, 0);
                    drawFrame(pb, f, frame);
                    CVPixelBufferUnlockBaseAddress(pb, 0);
                    dispatch_async(t.queue, ^{
                        appendTimed(t, ^BOOL { return [t.adaptor appendPixelBuffer:pb withPresentationTime:pts]; });
                        CVPixelBufferRelease(pb);
                    });
                }
            });
            [timers addObject:timer];
        };

        addVideo(@"screen", (VideoFormat){sw, sh, fps, nv12}, screenBitrate);
        if (cw > 0) addVideo(@"camera", (VideoFormat){cw, ch, fps, YES}, cameraBitrate);

        if (mic) {
            NSDictionary *aac = @{
                AVFormatIDKey : @(kAudioFormatMPEG4AAC),
                AVSampleRateKey : @(kAudioRate),
                AVNumberOfChannelsKey : @1,
                AVEncoderBitRateKey : @128000,
            };
            Track *t = makeTrack(out, @"mic", fileType, fragment, AVMediaTypeAudio, aac);
            [tracks addObject:t];
            AudioStreamBasicDescription asbd = {
                .mSampleRate = kAudioRate, .mFormatID = kAudioFormatLinearPCM,
                .mFormatFlags = kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked,
                .mBytesPerPacket = 4, .mFramesPerPacket = 1, .mBytesPerFrame = 4,
                .mChannelsPerFrame = 1, .mBitsPerChannel = 32,
            };
            CMAudioFormatDescriptionRef fmt = NULL;
            CMAudioFormatDescriptionCreate(NULL, &asbd, 0, NULL, 0, NULL, NULL, &fmt);
            __block int64_t sample = 0;
            dispatch_source_t timer = dispatch_source_create(DISPATCH_SOURCE_TYPE_TIMER, 0, 0, timerQueue);
            dispatch_source_set_timer(timer, dispatch_time(DISPATCH_TIME_NOW, 200 * NSEC_PER_MSEC), NSEC_PER_SEC / 100, 0);
            dispatch_source_set_event_handler(timer, ^{
                if (stopping) return;
                CMTime now = hostNow();
                while (true) {
                    CMTime pts = CMTimeAdd(t0, CMTimeMake(sample, kAudioRate));
                    if (CMTimeCompare(CMTimeAdd(pts, CMTimeMake(kAudioChunk, kAudioRate)), now) > 0) break;
                    size_t bytes = kAudioChunk * sizeof(float);
                    CMBlockBufferRef block = NULL;
                    CMBlockBufferCreateWithMemoryBlock(NULL, NULL, bytes, NULL, NULL, 0, bytes, kCMBlockBufferAssureMemoryNowFlag, &block);
                    float *pcm = NULL;
                    CMBlockBufferGetDataPointer(block, 0, NULL, NULL, (char **)&pcm);
                    for (int i = 0; i < kAudioChunk; i++) {
                        int64_t s = sample + i;
                        pcm[i] = (s % kAudioRate) < kAudioRate / 200 ? 0.8f : 0.0f; // 5 ms click per second
                    }
                    CMSampleBufferRef sb = NULL;
                    CMAudioSampleBufferCreateReadyWithPacketDescriptions(NULL, block, fmt, kAudioChunk, pts, NULL, &sb);
                    CFRelease(block);
                    sample += kAudioChunk;
                    dispatch_async(t.queue, ^{
                        appendTimed(t, ^BOOL { return [t.input appendSampleBuffer:sb]; });
                        CFRelease(sb);
                    });
                }
            });
            [timers addObject:timer];
        }

        for (Track *t in tracks) startTrack(t, t0);
        if (CMTimeCompare(hostNow(), t0) > 0) fprintf(stderr, "warning: setup took longer than the 2 s lead-in\n");

        for (dispatch_source_t timer in timers) dispatch_resume(timer);
        [NSThread sleepForTimeInterval:CMTimeGetSeconds(CMTimeSubtract(t0, hostNow()))];
        double wall0 = nowMs(), proc0 = processCpuSeconds();
        tot1 = cpuTicks(&idle1);
        fprintf(stderr, "recording %.0f s into %s (pid %d)\n", seconds, out.UTF8String, getpid());
        [NSThread sleepForTimeInterval:seconds];
        stopping = YES;
        for (dispatch_source_t timer in timers) dispatch_source_cancel(timer);
        dispatch_sync(timerQueue, ^{});
        uint64_t tot2 = cpuTicks(&idle2);
        double wall = (nowMs() - wall0) / 1000, proc = processCpuSeconds() - proc0;

        dispatch_group_t done = dispatch_group_create();
        for (Track *t in tracks) {
            dispatch_group_enter(done);
            dispatch_async(t.queue, ^{
                [t.input markAsFinished];
                [t.writer finishWritingWithCompletionHandler:^{ dispatch_group_leave(done); }];
            });
        }
        dispatch_group_wait(done, DISPATCH_TIME_FOREVER);

        int ncpu = (int)[NSProcessInfo processInfo].activeProcessorCount;
        double busy = 1.0 - (double)(idle2 - idle1) / (double)(tot2 - tot1);
        printf("{\n  \"seconds\": %.2f,\n  \"logical_cpus\": %d,\n", wall, ncpu);
        printf("  \"process_cpu_percent_of_one_core\": %.1f,\n", 100 * proc / wall);
        printf("  \"system_busy_percent_all_cores\": %.1f,\n  \"system_baseline_busy_percent\": %.1f,\n", 100 * busy, 100 * baselineBusy);
        printf("  \"tracks\": [\n");
        for (NSUInteger i = 0; i < tracks.count; i++) {
            Track *t = tracks[i];
            printf("    {\"name\": \"%s\", \"status\": %ld, \"produced\": %lld, \"appended\": %lld, \"dropped_not_ready\": %lld, \"append_failed\": %lld, \"append_ms_avg\": %.3f, \"append_ms_max\": %.2f, \"max_late_ms\": %.2f%s}%s\n",
                   t.name.UTF8String, (long)t.writer.status, t.produced, t.appended, t.droppedNotReady, t.appendFailed,
                   t.appended ? t.sumAppendMs / t.appended : 0, t.maxAppendMs, [jitter[t.name] doubleValue],
                   t.writer.error ? [[NSString stringWithFormat:@", \"error\": \"%@\"", t.writer.error.localizedDescription] UTF8String] : "",
                   i + 1 < tracks.count ? "," : "");
        }
        printf("  ]\n}\n");
    }
    return 0;
}
