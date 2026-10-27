// KreoKit Rec's macOS recorder; the interface and its rules are in recorder.h, the
// measurements behind every choice in docs/capture-research.md.
//
// Threads: every capture callback, the frame pacer, the audio cutter and the pause state
// run on one serial queue (`q`); each file has its own writer queue, so a slow encoder
// never blocks capture. Times are host-clock seconds (synthetic_fast: the generator's own
// clock).

#include "recorder.h"

#import <AVFoundation/AVFoundation.h>
#import <CoreImage/CoreImage.h>
#import <ImageIO/ImageIO.h>
#import <CoreGraphics/CoreGraphics.h>
#import <CoreMedia/CoreMedia.h>
#import <CoreVideo/CoreVideo.h>
#import <Foundation/Foundation.h>
#import <ScreenCaptureKit/ScreenCaptureKit.h>
#include <fcntl.h>
#include <math.h>
#include <pthread.h>
#include <stdatomic.h>
#include <unistd.h>

static const int kRate = 48000;
// How long a slot waits for its frame to arrive (capture latency) before it is written.
static const double kAllowance = 0.08;
// Up to this much media waits in front of each encoder; beyond it, new media is dropped.
static const double kQueueSeconds = 1.0;
// A frame exactly on a slot boundary belongs to the next slot (synthetic frames land
// exactly there; without a margin, rounding would decide).
static const double kTie = 1e-6;
// A small gap or overlap in the microphone's timestamps (jitter) is ignored up to this.
static const int64_t kAudioTolerance = 96; // 2 ms

static double hostSeconds(void) { return CMTimeGetSeconds(CMClockGetTime(CMClockGetHostTimeClock())); }

static void setError(char *error, size_t len, NSString *message) {
    if (error && len) snprintf(error, len, "%s", message.UTF8String);
}

// ---------------------------------------------------------------------------------------
// One file with one track, fed through a queue of at most kQueueSeconds.

@interface KRItem : NSObject {
  @public
    CVPixelBufferRef pb;
    CMSampleBufferRef sb;
    int64_t index; // video: the frame's slot
    int64_t samples;
}
@end
@implementation KRItem
- (void)dealloc {
    if (pb) CVPixelBufferRelease(pb);
    if (sb) CFRelease(sb);
}
@end

@interface KRWriter : NSObject {
  @public
    AVAssetWriter *writer;
    AVAssetWriterInput *input;
    AVAssetWriterInputPixelBufferAdaptor *adaptor;
    dispatch_queue_t queue;
    NSMutableArray<KRItem *> *pending;
    int fps;
    BOOL draining;
    int fd; // for fsync
    _Atomic int64_t queued; // video: frames; audio: samples
    int64_t limit;
    _Atomic int64_t dropped;
    NSString *path;
}
@end

@implementation KRWriter

+ (instancetype)writerAt:(NSString *)path video:(BOOL)video settings:(NSDictionary *)settings fps:(int)fps realtime:(BOOL)realtime error:(NSString **)error {
    KRWriter *w = [KRWriter new];
    w->path = path;
    w->fps = fps;
    [[NSFileManager defaultManager] removeItemAtPath:path error:nil];
    NSError *e = nil;
    w->writer = [AVAssetWriter assetWriterWithURL:[NSURL fileURLWithPath:path] fileType:AVFileTypeMPEG4 error:&e];
    if (!w->writer) {
        *error = e.localizedDescription;
        return nil;
    }
    // Fragmented MP4: a crash keeps everything up to the last fragment (measured: 0.4-1.1 s
    // lost with 1 s fragments and keyframes).
    w->writer.movieFragmentInterval = CMTimeMakeWithSeconds(1.0, 600);
    w->input = [AVAssetWriterInput assetWriterInputWithMediaType:video ? AVMediaTypeVideo : AVMediaTypeAudio outputSettings:settings];
    w->input.expectsMediaDataInRealTime = realtime;
    if (![w->writer canAddInput:w->input]) {
        *error = @"the encoder refused the settings";
        return nil;
    }
    [w->writer addInput:w->input];
    if (video) w->adaptor = [AVAssetWriterInputPixelBufferAdaptor assetWriterInputPixelBufferAdaptorWithAssetWriterInput:w->input sourcePixelBufferAttributes:nil];
    w->queue = dispatch_queue_create("kreokit.rec.writer", DISPATCH_QUEUE_SERIAL);
    w->pending = [NSMutableArray array];
    w->limit = video ? (int64_t)(fps * kQueueSeconds) : (int64_t)(kRate * kQueueSeconds);
    if (![w->writer startWriting]) {
        *error = w->writer.error.localizedDescription ?: @"the file could not be created";
        return nil;
    }
    [w->writer startSessionAtSourceTime:kCMTimeZero];
    w->fd = open(path.fileSystemRepresentation, O_RDONLY);
    return w;
}

- (int64_t)sizeOf:(KRItem *)item {
    return item->pb ? 1 : item->samples;
}

// Takes ownership of `item`. Returns NO when it was dropped (queue full).
- (BOOL)enqueue:(KRItem *)item {
    int64_t size = [self sizeOf:item];
    if (atomic_load(&queued) + size > limit) {
        atomic_fetch_add(&dropped, size);
        return NO;
    }
    atomic_fetch_add(&queued, size);
    dispatch_async(queue, ^{
        [self->pending addObject:item];
        [self drain];
    });
    return YES;
}

// On `queue`: appends what the encoder takes now, and checks back shortly for the rest.
- (void)drain {
    while (pending.count && input.readyForMoreMediaData) {
        KRItem *item = pending.firstObject;
        [pending removeObjectAtIndex:0];
        atomic_fetch_sub(&queued, [self sizeOf:item]);
        BOOL ok = item->pb ? [adaptor appendPixelBuffer:item->pb withPresentationTime:CMTimeMake(item->index, fps)] : [input appendSampleBuffer:item->sb];
        if (!ok) atomic_fetch_add(&dropped, [self sizeOf:item]);
    }
    if (pending.count && !draining && writer.status == AVAssetWriterStatusWriting) {
        draining = YES;
        dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 3 * NSEC_PER_MSEC), queue, ^{
            self->draining = NO;
            [self drain];
        });
    }
}

- (void)sync {
    if (fd >= 0) fsync(fd);
}

// Writes what is still queued (waiting up to 10 s for the encoder) and closes the file.
- (void)finish:(dispatch_group_t)group {
    dispatch_group_enter(group);
    dispatch_async(queue, ^{
        double until = hostSeconds() + 10;
        while (self->pending.count && hostSeconds() < until && self->writer.status == AVAssetWriterStatusWriting) {
            [self drain];
            if (self->pending.count) usleep(2000);
        }
        [self->input markAsFinished];
        [self->writer finishWritingWithCompletionHandler:^{
            if (self->fd >= 0) {
                fsync(self->fd);
                close(self->fd);
                self->fd = -1;
            }
            dispatch_group_leave(group);
        }];
    });
}

- (NSString *)failure {
    return writer.status == AVAssetWriterStatusFailed ? (writer.error.localizedDescription ?: @"unknown error") : nil;
}

@end

// ---------------------------------------------------------------------------------------

@interface KRFrame : NSObject {
  @public
    CVPixelBufferRef pb;
    double t;
}
@end
@implementation KRFrame
- (void)dealloc {
    CVPixelBufferRelease(pb);
}
@end

// One picture track (screen or camera): frames arrive at their own pace, one is kept per
// output slot.
@interface KRPacer : NSObject {
  @public
    KRWriter *w;
    BOOL camera;
    int64_t slot;
    NSMutableArray<KRFrame *> *frames; // captured, waiting for their slot, by time
    KRFrame *current;                  // the picture of the last written slot
    double lastBoundary;
    int64_t written, fresh, repeated, late, lost;
}
@end
@implementation KRPacer
+ (instancetype)pacerWith:(KRWriter *)w camera:(BOOL)camera {
    KRPacer *p = [KRPacer new];
    p->w = w;
    p->camera = camera;
    p->frames = [NSMutableArray array];
    p->lastBoundary = -INFINITY;
    return p;
}
@end

static NSDictionary *h264Settings(int w, int h, int fps, int bitrate) {
    return @{
        AVVideoCodecKey : AVVideoCodecTypeH264,
        AVVideoWidthKey : @(w),
        AVVideoHeightKey : @(h),
        AVVideoColorPropertiesKey : @{
            AVVideoColorPrimariesKey : AVVideoColorPrimaries_ITU_R_709_2,
            AVVideoTransferFunctionKey : AVVideoTransferFunction_ITU_R_709_2,
            AVVideoYCbCrMatrixKey : AVVideoYCbCrMatrix_ITU_R_709_2,
        },
        AVVideoCompressionPropertiesKey : @{
            AVVideoAverageBitRateKey : @(bitrate),
            AVVideoProfileLevelKey : AVVideoProfileLevelH264HighAutoLevel,
            AVVideoExpectedSourceFrameRateKey : @(fps),
            AVVideoMaxKeyFrameIntervalKey : @(fps),
            AVVideoMaxKeyFrameIntervalDurationKey : @1,
            AVVideoAllowFrameReorderingKey : @NO, // no B-frames
        },
    };
}

static NSDictionary *aacSettings(void) {
    return @{AVFormatIDKey : @(kAudioFormatMPEG4AAC), AVSampleRateKey : @(kRate), AVNumberOfChannelsKey : @1, AVEncoderBitRateKey : @128000};
}

// Timestamps from an AVCaptureSession are on its own clock; everything else uses host time.
static double hostTimeOf(CMSampleBufferRef sb, AVCaptureSession *session) {
    CMClockRef clock = NULL;
    if (@available(macOS 12.3, *)) clock = session.synchronizationClock;
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
    if (!clock) clock = session.masterClock;
#pragma clang diagnostic pop
    CMTime pts = CMSampleBufferGetPresentationTimeStamp(sb);
    if (clock) pts = CMSyncConvertTime(pts, clock, CMClockGetHostTimeClock());
    return CMTimeGetSeconds(pts);
}

// Float samples (48 kHz mono, as the audio output is set up) of a microphone buffer.
static void withSamples(CMSampleBufferRef sb, void (^body)(const float *, int64_t)) {
    AudioBufferList list;
    CMBlockBufferRef block = NULL;
    if (CMSampleBufferGetAudioBufferListWithRetainedBlockBuffer(sb, NULL, &list, sizeof list, NULL, NULL, 0, &block) != noErr) return;
    body((const float *)list.mBuffers[0].mData, (int64_t)(list.mBuffers[0].mDataByteSize / sizeof(float)));
    CFRelease(block);
}

static AVCaptureDevice *micDevice(const char *mic_id) {
    if (mic_id && *mic_id) {
        AVCaptureDevice *d = [AVCaptureDevice deviceWithUniqueID:@(mic_id)];
        if (d) return d;
    }
    return [AVCaptureDevice defaultDeviceWithMediaType:AVMediaTypeAudio];
}

static AVCaptureDevice *cameraDevice(const char *camera_id) {
    if (camera_id && *camera_id) {
        AVCaptureDevice *d = [AVCaptureDevice deviceWithUniqueID:@(camera_id)];
        if (d) return d;
    }
    return [AVCaptureDevice defaultDeviceWithMediaType:AVMediaTypeVideo];
}

// The camera format the recorder uses: the largest up to 720 pixels high that runs at
// 30 fps (kr_cameras_json reports its size).
static AVCaptureDeviceFormat *cameraFormat(AVCaptureDevice *dev) {
    AVCaptureDeviceFormat *best = nil;
    int64_t bestArea = 0;
    for (AVCaptureDeviceFormat *f in dev.formats) {
        CMVideoDimensions d = CMVideoFormatDescriptionGetDimensions(f.formatDescription);
        if (d.height > 720) continue;
        BOOL runs30 = NO;
        for (AVFrameRateRange *r in f.videoSupportedFrameRateRanges)
            if (r.minFrameRate <= 30.01 && r.maxFrameRate >= 29.99) runs30 = YES;
        int64_t area = (int64_t)d.width * d.height;
        if (runs30 && area > bestArea) best = f, bestArea = area;
    }
    return best;
}

static NSDictionary *floatMonoSettings(void) {
    return @{
        AVFormatIDKey : @(kAudioFormatLinearPCM), AVSampleRateKey : @(kRate), AVNumberOfChannelsKey : @1,
        AVLinearPCMBitDepthKey : @32, AVLinearPCMIsFloatKey : @YES, AVLinearPCMIsNonInterleaved : @NO,
    };
}

// ---------------------------------------------------------------------------------------

@interface KRRecorder : NSObject <AVCaptureAudioDataOutputSampleBufferDelegate, AVCaptureVideoDataOutputSampleBufferDelegate> {
  @public
    KrRecConfig cfg;
    NSString *dir, *micId, *cameraId;
    dispatch_queue_t q;
    double t0;
    double pausedTotal, pauseStart; // pauseStart < 0: not paused
    NSMutableArray<NSArray<NSNumber *> *> *pauses; // [P, R] host times, for the audio
    double stopAt;
    KRPacer *screenP, *cameraP;
    // Audio.
    int64_t audioNext; // next output sample
    float peakNow, peakPrev;
    int64_t peakCount;
    CMAudioFormatDescriptionRef pcmFormat;
    KrRecStatus st;
    NSString *error;
    KRWriter *screenW, *cameraW, *micW;
    // Sources.
    CGDisplayStreamRef cgd;
    id scStream, scOutput;
    AVCaptureSession *micSession, *cameraSession;
    dispatch_source_t timer, syncTimer;
    // Synthetic sources.
    _Atomic double synthNow;
    dispatch_semaphore_t synthDone;
    CVPixelBufferPoolRef synthPool, synthCameraPool;
}
@end

@interface KRRecorder (Pacer)
- (void)ingest:(CVPixelBufferRef)pb at:(double)t into:(KRPacer *)p;
- (void)audio:(const float *)s count:(int64_t)n at:(double)h;
@end

#pragma mark - SCK output (macOS 12.3+)

API_AVAILABLE(macos(12.3))
@interface KRStreamOutput : NSObject <SCStreamOutput, SCStreamDelegate>
@property(nonatomic, weak) KRRecorder *rec;
@end

@implementation KRStreamOutput
// Frames whose status isn't "complete" (idle: nothing changed) carry no new picture.
- (void)stream:(SCStream *)stream didOutputSampleBuffer:(CMSampleBufferRef)sb ofType:(SCStreamOutputType)type {
    if (type != SCStreamOutputTypeScreen) return;
    CFArrayRef attachments = CMSampleBufferGetSampleAttachmentsArray(sb, false);
    if (!attachments || CFArrayGetCount(attachments) == 0) return;
    NSDictionary *info = (__bridge NSDictionary *)CFArrayGetValueAtIndex(attachments, 0);
    CVPixelBufferRef pb = CMSampleBufferGetImageBuffer(sb);
    if ([info[SCStreamFrameInfoStatus] integerValue] != SCFrameStatusComplete || !pb) return;
    KRRecorder *rec = self.rec;
    [rec ingest:pb at:CMTimeGetSeconds(CMSampleBufferGetPresentationTimeStamp(sb)) into:rec->screenP];
}

- (void)stream:(SCStream *)stream didStopWithError:(NSError *)error {
    KRRecorder *rec = self.rec;
    dispatch_async(rec->q, ^{
        if (!rec->error) rec->error = [NSString stringWithFormat:@"Screen recording stopped (%@). Your recording up to now is kept.", error.localizedDescription];
        rec->st.failed = 1;
    });
}
@end

@implementation KRRecorder

- (double)now {
    return cfg.synthetic_fast ? atomic_load(&synthNow) : hostSeconds();
}

- (void)fail:(NSString *)message {
    if (!error) error = message;
    st.failed = 1;
}

#pragma mark Frame pacer

// The host time of a track's next output slot, and its boundary: the slot takes the newest
// frame captured before slot time + half a frame.
- (double)slotTime:(KRPacer *)p {
    return t0 + (double)p->slot / cfg.fps + pausedTotal;
}

- (double)nextBoundary:(KRPacer *)p {
    double hk = [self slotTime:p];
    if (pauseStart >= 0 && hk >= pauseStart) return INFINITY;
    return hk + 0.5 / cfg.fps;
}

- (void)ingest:(CVPixelBufferRef)pb at:(double)t into:(KRPacer *)p {
    if (!p) return;
    if (t < p->lastBoundary - kTie) {
        p->late++;
        if (p->current && t <= p->current->t) return;
    }
    KRFrame *f = [KRFrame new];
    f->pb = CVPixelBufferRetain(pb);
    f->t = t;
    NSMutableArray<KRFrame *> *frames = p->frames;
    NSUInteger i = frames.count;
    while (i > 0 && frames[i - 1]->t > t) i--;
    [frames insertObject:f atIndex:i];
    // Only the newest frame before the next slot's boundary can still be used.
    double nb = [self nextBoundary:p];
    while (frames.count >= 2 && frames[1]->t < nb - kTie) [frames removeObjectAtIndex:0];
}

// Writes every slot of every picture track whose boundary is before `limit` (and before a
// pause or the stop).
- (void)emitUpTo:(double)limit {
    if (screenP) [self emit:screenP upTo:limit];
    if (cameraP) [self emit:cameraP upTo:limit];
}

- (void)emit:(KRPacer *)p upTo:(double)limit {
    while (!st.failed) {
        double hk = [self slotTime:p];
        if (pauseStart >= 0 && hk >= pauseStart) break;
        if (hk >= stopAt) break;
        double boundary = hk + 0.5 / cfg.fps;
        if (boundary > limit) break;
        KRFrame *pick = nil;
        while (p->frames.count && p->frames[0]->t < boundary - kTie) {
            pick = p->frames[0];
            [p->frames removeObjectAtIndex:0];
        }
        if (pick) {
            p->current = pick;
            p->fresh++;
        } else if (p->current) {
            p->repeated++;
        } else {
            if (hk - t0 > 3)
                [self fail:p->camera ? @"No picture arrived from the camera. Check that KreoKit Rec may use the camera and that no other app is using it, then try again."
                                     : @"No picture arrived from the screen. Check that KreoKit Rec may record the screen, then try again."];
            break; // waits for the first picture
        }
        KRItem *item = [KRItem new];
        item->pb = CVPixelBufferRetain(p->current->pb);
        item->index = p->slot;
        [p->w enqueue:item]; // a full queue drops it (the writer counts it)
        p->written++;
        p->slot++;
        p->lastBoundary = boundary;
    }
}

#pragma mark Audio

- (void)levelOf:(const float *)s count:(int64_t)n {
    for (int64_t i = 0; i < n; i++) {
        float a = fabsf(s[i]);
        if (a > peakNow) peakNow = a;
        if (++peakCount >= kRate / 10) {
            peakPrev = peakNow;
            peakNow = 0;
            peakCount = 0;
        }
    }
}

- (void)writeAudio:(const float *)s count:(int64_t)n {
    if (!micW || n <= 0) return;
    size_t bytes = (size_t)n * sizeof(float);
    CMBlockBufferRef block = NULL;
    if (CMBlockBufferCreateWithMemoryBlock(NULL, NULL, bytes, NULL, NULL, 0, bytes, kCMBlockBufferAssureMemoryNowFlag, &block) != noErr) return;
    if (s) CMBlockBufferReplaceDataBytes(s, block, 0, bytes);
    else CMBlockBufferFillDataBytes(0, block, 0, bytes);
    CMSampleBufferRef sb = NULL;
    CMAudioSampleBufferCreateReadyWithPacketDescriptions(NULL, block, pcmFormat, (CMItemCount)n, CMTimeMake(audioNext, kRate), NULL, &sb);
    CFRelease(block);
    if (!sb) return;
    KRItem *item = [KRItem new];
    item->sb = sb;
    item->samples = n;
    // A full queue drops the samples (the writer counts them) but keeps their place, so
    // what follows stays in sync.
    if ([micW enqueue:item]) st.audio_samples += n;
    audioNext += n;
}

// Time removed by the pauses that ended before host time h.
- (double)pausedBefore:(double)h {
    double sum = 0;
    for (NSArray<NSNumber *> *p in pauses)
        if (p[1].doubleValue <= h) sum += p[1].doubleValue - p[0].doubleValue;
    return sum;
}

- (BOOL)keepsSampleAt:(double)h {
    if (h < t0 || h >= stopAt) return NO;
    if (pauseStart >= 0 && h >= pauseStart) return NO;
    for (NSArray<NSNumber *> *p in pauses)
        if (h >= p[0].doubleValue && h < p[1].doubleValue) return NO;
    return YES;
}

// Microphone samples starting at host time h: cut at the start, the pauses and the stop,
// and placed on the output timeline right after the previous ones. A real gap (the
// microphone sent nothing) becomes silence, so the sound never slides against the picture.
- (void)audio:(const float *)s count:(int64_t)n at:(double)h {
    [self levelOf:s count:n];
    if (!micW) return;
    int64_t i = 0;
    while (i < n) {
        while (i < n && ![self keepsSampleAt:h + (double)i / kRate]) i++;
        int64_t start = i;
        while (i < n && [self keepsSampleAt:h + (double)i / kRate]) i++;
        if (i == start) break;
        double hs = h + (double)start / kRate;
        int64_t pos = llround((hs - t0 - [self pausedBefore:hs]) * kRate);
        int64_t skip = 0;
        if (pos > audioNext + kAudioTolerance) {
            int64_t gap = pos - audioNext;
            st.audio_filled += gap;
            while (gap > 0) {
                int64_t part = MIN(gap, kRate / 10);
                [self writeAudio:NULL count:part];
                gap -= part;
            }
        } else if (pos < audioNext - kAudioTolerance) {
            skip = MIN(audioNext - pos, i - start);
            st.audio_dropped += skip;
        }
        [self writeAudio:s + start + skip count:i - start - skip];
    }
}

- (void)captureOutput:(AVCaptureOutput *)output didOutputSampleBuffer:(CMSampleBufferRef)sb fromConnection:(AVCaptureConnection *)connection {
    if ([output isKindOfClass:AVCaptureVideoDataOutput.class]) {
        CVPixelBufferRef pb = CMSampleBufferGetImageBuffer(sb);
        if (pb) [self ingest:pb at:hostTimeOf(sb, cameraSession) into:cameraP];
        return;
    }
    double h = hostTimeOf(sb, micSession);
    withSamples(sb, ^(const float *s, int64_t n) { [self audio:s count:n at:h]; });
}

// The camera's own queue was full (we were too slow to take a picture).
- (void)captureOutput:(AVCaptureOutput *)output didDropSampleBuffer:(CMSampleBufferRef)sb fromConnection:(AVCaptureConnection *)connection {
    if ([output isKindOfClass:AVCaptureVideoDataOutput.class] && cameraP) cameraP->lost++;
}

#pragma mark Synthetic sources

// The test pattern (also drawn by the bench window and on the synthetic camera;
// src/testpattern.rs reads it back): a
// 32-bit frame counter as black/white squares in the top row (34 cells across the
// picture), and a white strip below it for one output frame from every whole second, when
// the mic clicks (one output frame long, so a slot always catches it).
static void drawPattern(CVPixelBufferRef pb, int64_t n, BOOL marker) {
    CVPixelBufferLockBaseAddress(pb, 0);
    size_t w = CVPixelBufferGetWidthOfPlane(pb, 0), h = CVPixelBufferGetHeightOfPlane(pb, 0);
    uint8_t *y = CVPixelBufferGetBaseAddressOfPlane(pb, 0);
    size_t ys = CVPixelBufferGetBytesPerRowOfPlane(pb, 0);
    for (size_t r = 0; r < h; r++) memset(y + r * ys, 128, w);
    uint8_t *uv = CVPixelBufferGetBaseAddressOfPlane(pb, 1);
    size_t uvs = CVPixelBufferGetBytesPerRowOfPlane(pb, 1);
    for (size_t r = 0; r < CVPixelBufferGetHeightOfPlane(pb, 1); r++) memset(uv + r * uvs, 128, CVPixelBufferGetWidthOfPlane(pb, 1) * 2);
    size_t cell = w / 34;
    for (int b = 0; b < 32; b++) {
        uint8_t v = ((n >> (31 - b)) & 1) ? 235 : 16;
        for (size_t r = cell / 2; r < cell / 2 + cell; r++) memset(y + r * ys + cell / 2 + b * cell, v, cell);
    }
    for (size_t r = cell * 2; r < cell * 4 && r < h; r++) memset(y + r * ys, marker ? 235 : 16, w);
    CVPixelBufferUnlockBaseAddress(pb, 0);
}

// Runs the generated screen (capture_fps), camera (fps, its counter = its frame number) and
// microphone (a 5 ms click every whole second from the start) until the stop. The screen's
// counter is the capture frame number since the start.
- (void)runSynthetic {
    int cfps = cfg.capture_fps;
    double base = t0 - 0.5;
    int64_t chunk = kRate / cfps;
    float *audio = calloc((size_t)chunk, sizeof(float));
    for (int64_t i = 0;; i++) {
        double t = base + (double)i / cfps;
        if (cfg.synthetic_fast) {
            atomic_store(&synthNow, t);
            // The encoder sets the pace: wait while half a second is queued.
            while ((screenW && atomic_load(&screenW->queued) > screenW->limit / 2) || (cameraW && atomic_load(&cameraW->queued) > cameraW->limit / 2) ||
                   (micW && atomic_load(&micW->queued) > micW->limit / 2))
                usleep(500);
        } else {
            double wait = t - hostSeconds();
            if (wait > 0) usleep((useconds_t)(wait * 1e6));
        }
        __block BOOL done = NO;
        dispatch_sync(q, ^{
            if (t > self->stopAt + 0.3) {
                done = YES;
                return;
            }
            int64_t n = llround((t - self->t0) * cfps);
            double since = t - self->t0;
            BOOL frozen = self->cfg.synthetic_static && since >= 0 && fmod(since, 10.0) >= 5 && fmod(since, 10.0) < 7;
            for (int64_t k = 0; k < chunk; k++) {
                double ts = t + (double)k / kRate - self->t0;
                double frac = ts - floor(ts);
                audio[k] = (ts >= 0 && frac < 0.005) ? 0.5f : 0.0f;
            }
            [self audio:audio count:chunk at:t];
            if (self->screenW && !frozen) {
                CVPixelBufferRef pb = NULL;
                CVPixelBufferPoolCreatePixelBuffer(NULL, self->synthPool, &pb);
                if (pb) {
                    drawPattern(pb, n < 0 ? 0 : n, n >= 0 && n % cfps < cfps / self->cfg.fps);
                    [self ingest:pb at:t into:self->screenP];
                    CVPixelBufferRelease(pb);
                }
            }
            if (self->cameraW && i % (cfps / self->cfg.fps) == 0) {
                int64_t c = llround((t - self->t0) * self->cfg.fps);
                CVPixelBufferRef pb = NULL;
                CVPixelBufferPoolCreatePixelBuffer(NULL, self->synthCameraPool, &pb);
                if (pb) {
                    drawPattern(pb, c < 0 ? 0 : c, c >= 0 && c % self->cfg.fps == 0);
                    [self ingest:pb at:t into:self->cameraP];
                    CVPixelBufferRelease(pb);
                }
            }
            if (self->cfg.synthetic_fast) [self emitUpTo:t - kAllowance];
        });
        if (done) break;
    }
    free(audio);
    dispatch_semaphore_signal(synthDone);
}

#pragma mark Sources

- (BOOL)startScreen:(NSString **)message {
    int api = cfg.screen_api;
    if (api == KR_SCREEN_AUTO) {
        if (@available(macOS 13.0, *)) api = KR_SCREEN_SCREENCAPTUREKIT;
        else api = KR_SCREEN_CGDISPLAYSTREAM;
    }
    CGDirectDisplayID display = cfg.display_id ? cfg.display_id : CGMainDisplayID();
    if (api == KR_SCREEN_CGDISPLAYSTREAM) {
        NSDictionary *props = @{
            (__bridge NSString *)kCGDisplayStreamMinimumFrameTime : @(1.0 / cfg.capture_fps),
            (__bridge NSString *)kCGDisplayStreamShowCursor : @YES,
            (__bridge NSString *)kCGDisplayStreamYCbCrMatrix : (__bridge NSString *)kCGDisplayStreamYCbCrMatrix_ITU_R_709_2,
            (__bridge NSString *)kCGDisplayStreamQueueDepth : @8,
        };
        __weak KRRecorder *weak = self;
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
        cgd = CGDisplayStreamCreateWithDispatchQueue(
            display, (size_t)cfg.width, (size_t)cfg.height, kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange, (__bridge CFDictionaryRef)props, q,
            ^(CGDisplayStreamFrameStatus status, uint64_t displayTime, IOSurfaceRef surface, CGDisplayStreamUpdateRef update) {
                KRRecorder *rec = weak;
                if (!rec || status != kCGDisplayStreamFrameStatusFrameComplete || !surface) return;
                CVPixelBufferRef pb = NULL;
                CVPixelBufferCreateWithIOSurface(NULL, surface, NULL, &pb);
                if (!pb) return;
                // displayTime is mach_absolute_time: the host clock.
                [rec ingest:pb at:CMTimeGetSeconds(CMClockMakeHostTimeFromSystemUnits(displayTime)) into:rec->screenP];
                CVPixelBufferRelease(pb);
            });
        if (!cgd || CGDisplayStreamStart(cgd) != kCGErrorSuccess) {
            *message = @"The screen can't be recorded. Allow KreoKit Rec under System Preferences > Security & Privacy > Privacy > Screen Recording, then restart the app.";
            return NO;
        }
#pragma clang diagnostic pop
        return YES;
    }
    if (@available(macOS 12.3, *)) {
        dispatch_semaphore_t got = dispatch_semaphore_create(0);
        __block SCShareableContent *content = nil;
        [SCShareableContent getShareableContentWithCompletionHandler:^(SCShareableContent *c, NSError *e) {
            content = c;
            dispatch_semaphore_signal(got);
        }];
        dispatch_semaphore_wait(got, dispatch_time(DISPATCH_TIME_NOW, 10 * NSEC_PER_SEC));
        SCDisplay *found = nil;
        for (SCDisplay *d in content.displays)
            if (d.displayID == display) found = d;
        if (!found) {
            *message = @"The screen can't be recorded. Allow KreoKit Rec under System Settings > Privacy & Security > Screen Recording, then restart the app.";
            return NO;
        }
        SCContentFilter *filter = [[SCContentFilter alloc] initWithDisplay:found excludingWindows:@[]];
        SCStreamConfiguration *c = [SCStreamConfiguration new];
        c.width = (size_t)cfg.width;
        c.height = (size_t)cfg.height;
        c.minimumFrameInterval = CMTimeMake(1, cfg.capture_fps);
        c.pixelFormat = kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange;
        c.colorMatrix = kCGDisplayStreamYCbCrMatrix_ITU_R_709_2;
        c.queueDepth = 8;
        c.showsCursor = YES;
        KRStreamOutput *out = [KRStreamOutput new];
        out.rec = self;
        scOutput = out;
        SCStream *stream = [[SCStream alloc] initWithFilter:filter configuration:c delegate:out];
        NSError *e = nil;
        if (![stream addStreamOutput:out type:SCStreamOutputTypeScreen sampleHandlerQueue:q error:&e]) {
            *message = [NSString stringWithFormat:@"The screen can't be recorded (%@).", e.localizedDescription];
            return NO;
        }
        dispatch_semaphore_t started = dispatch_semaphore_create(0);
        __block NSError *startError = nil;
        [stream startCaptureWithCompletionHandler:^(NSError *err) {
            startError = err;
            dispatch_semaphore_signal(started);
        }];
        dispatch_semaphore_wait(started, dispatch_time(DISPATCH_TIME_NOW, 10 * NSEC_PER_SEC));
        if (startError) {
            *message = [NSString stringWithFormat:@"The screen can't be recorded (%@). Check Screen Recording in System Settings > Privacy & Security, then restart the app.", startError.localizedDescription];
            return NO;
        }
        scStream = stream;
        return YES;
    }
    *message = @"This way of recording the screen needs macOS 12.3 or newer.";
    return NO;
}

- (BOOL)startMic:(NSString **)message {
    AVCaptureDevice *dev = micDevice(micId.UTF8String);
    NSError *e = nil;
    AVCaptureDeviceInput *input = dev ? [AVCaptureDeviceInput deviceInputWithDevice:dev error:&e] : nil;
    if (!input) {
        *message = dev ? [NSString stringWithFormat:@"The microphone can't be opened (%@). Check that KreoKit Rec may use it in System Preferences > Security & Privacy > Privacy > Microphone.", e.localizedDescription]
                       : @"No microphone found. Connect one, or record without the microphone.";
        return NO;
    }
    micSession = [AVCaptureSession new];
    [micSession beginConfiguration];
    [micSession addInput:input];
    AVCaptureAudioDataOutput *out = [AVCaptureAudioDataOutput new];
    out.audioSettings = floatMonoSettings();
    [out setSampleBufferDelegate:self queue:q];
    [micSession addOutput:out];
    [micSession commitConfiguration];
    [micSession startRunning];
    if (!micSession.running) {
        *message = @"The microphone didn't start. Check that KreoKit Rec may use it in System Preferences > Security & Privacy > Privacy > Microphone.";
        return NO;
    }
    return YES;
}

- (BOOL)startCamera:(NSString **)message {
    AVCaptureDevice *dev = cameraDevice(cameraId.UTF8String);
    NSError *e = nil;
    AVCaptureDeviceInput *input = dev ? [AVCaptureDeviceInput deviceInputWithDevice:dev error:&e] : nil;
    if (!input) {
        *message = dev ? [NSString stringWithFormat:@"The camera can't be opened (%@). Check that KreoKit Rec may use it in System Preferences > Security & Privacy > Privacy > Camera, and that no other app is using it.", e.localizedDescription]
                       : @"No camera found. Connect one, or record without the camera.";
        return NO;
    }
    cameraSession = [AVCaptureSession new];
    [cameraSession beginConfiguration];
    [cameraSession addInput:input];
    AVCaptureVideoDataOutput *out = [AVCaptureVideoDataOutput new];
    // NV12 at the output size: the camera's scaler does the work, not our process.
    out.videoSettings = @{
        (id)kCVPixelBufferPixelFormatTypeKey : @(kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange),
        (id)kCVPixelBufferWidthKey : @(cfg.camera_width),
        (id)kCVPixelBufferHeightKey : @(cfg.camera_height),
    };
    [out setSampleBufferDelegate:self queue:q];
    [cameraSession addOutput:out];
    [cameraSession commitConfiguration];
    // A fixed frame rate (min = max frame duration): otherwise the camera halves it in low
    // light. The lock is held through startRunning so the session keeps the setting.
    BOOL locked = [dev lockForConfiguration:nil];
    if (locked) {
        AVCaptureDeviceFormat *format = cameraFormat(dev);
        if (format) dev.activeFormat = format;
        CMTime frame = CMTimeMake(1, cfg.fps);
        for (AVFrameRateRange *r in dev.activeFormat.videoSupportedFrameRateRanges) {
            if (r.minFrameRate <= cfg.fps + 0.01 && r.maxFrameRate >= cfg.fps - 0.01) {
                dev.activeVideoMinFrameDuration = frame;
                dev.activeVideoMaxFrameDuration = frame;
                break;
            }
        }
    }
    [cameraSession startRunning];
    if (locked) [dev unlockForConfiguration];
    if (!cameraSession.running) {
        *message = @"The camera didn't start. Check that KreoKit Rec may use it in System Preferences > Security & Privacy > Privacy > Camera, and that no other app is using it.";
        return NO;
    }
    return YES;
}

- (void)stopSources {
    if (timer) dispatch_source_cancel(timer), timer = nil;
    if (syncTimer) dispatch_source_cancel(syncTimer), syncTimer = nil;
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
    if (cgd) {
        CGDisplayStreamStop(cgd);
        CFRelease(cgd);
        cgd = NULL;
    }
#pragma clang diagnostic pop
    if (@available(macOS 12.3, *)) {
        if (scStream) {
            dispatch_semaphore_t stopped = dispatch_semaphore_create(0);
            [(SCStream *)scStream stopCaptureWithCompletionHandler:^(NSError *e) { dispatch_semaphore_signal(stopped); }];
            dispatch_semaphore_wait(stopped, dispatch_time(DISPATCH_TIME_NOW, 3 * NSEC_PER_SEC));
            scStream = nil;
        }
    }
    [micSession stopRunning];
    micSession = nil;
    [cameraSession stopRunning];
    cameraSession = nil;
    // The generator runs on to 0.3 s after the stop by itself, so the last slots get their
    // frames as with a real screen.
    if (synthDone) {
        dispatch_semaphore_wait(synthDone, DISPATCH_TIME_FOREVER);
        synthDone = nil;
    }
}

- (void)dealloc {
    if (pcmFormat) CFRelease(pcmFormat);
    if (synthPool) CVPixelBufferPoolRelease(synthPool);
    if (synthCameraPool) CVPixelBufferPoolRelease(synthCameraPool);
}

@end

#pragma mark - C interface

void *kr_rec_start(const KrRecConfig *config, char *error, size_t error_len) {
    @autoreleasepool {
        KRRecorder *rec = [KRRecorder new];
        rec->cfg = *config;
        rec->dir = @(config->dir);
        rec->micId = config->mic_id ? @(config->mic_id) : @"";
        rec->cameraId = config->camera_id ? @(config->camera_id) : @"";
        rec->cfg.dir = NULL;
        rec->cfg.mic_id = NULL;
        rec->cfg.camera_id = NULL;
        if (rec->cfg.capture_fps < rec->cfg.fps) rec->cfg.capture_fps = rec->cfg.fps;
        rec->q = dispatch_queue_create("kreokit.rec.capture", dispatch_queue_attr_make_with_qos_class(DISPATCH_QUEUE_SERIAL, QOS_CLASS_USER_INTERACTIVE, 0));
        rec->pauses = [NSMutableArray array];
        rec->pauseStart = -1;
        rec->stopAt = INFINITY;
        AudioStreamBasicDescription asbd = {kRate, kAudioFormatLinearPCM, kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked, 4, 1, 4, 1, 32, 0};
        CMAudioFormatDescriptionCreate(NULL, &asbd, 0, NULL, 0, NULL, NULL, &rec->pcmFormat);

        BOOL realtime = !config->synthetic_fast;
        NSString *message = nil;
        if (config->screen) {
            rec->screenW = [KRWriter writerAt:[rec->dir stringByAppendingPathComponent:@"screen.mp4"] video:YES
                                     settings:h264Settings(config->width, config->height, config->fps, config->video_bitrate)
                                          fps:config->fps realtime:realtime error:&message];
            if (!rec->screenW) {
                setError(error, error_len, [NSString stringWithFormat:@"The screen recording file can't be created (%@). Check that the disk has space.", message]);
                return NULL;
            }
            rec->screenP = [KRPacer pacerWith:rec->screenW camera:NO];
        }
        if (config->camera) {
            rec->cameraW = [KRWriter writerAt:[rec->dir stringByAppendingPathComponent:@"camera.mp4"] video:YES
                                     settings:h264Settings(config->camera_width, config->camera_height, config->fps, config->camera_bitrate)
                                          fps:config->fps realtime:realtime error:&message];
            if (!rec->cameraW) {
                setError(error, error_len, [NSString stringWithFormat:@"The camera recording file can't be created (%@). Check that the disk has space.", message]);
                return NULL;
            }
            rec->cameraP = [KRPacer pacerWith:rec->cameraW camera:YES];
        }
        if (config->mic) {
            rec->micW = [KRWriter writerAt:[rec->dir stringByAppendingPathComponent:@"mic.mp4"] video:NO settings:aacSettings() fps:config->fps realtime:realtime error:&message];
            if (!rec->micW) {
                setError(error, error_len, [NSString stringWithFormat:@"The microphone recording file can't be created (%@). Check that the disk has space.", message]);
                return NULL;
            }
        }

        double now = hostSeconds();
        rec->t0 = now + 0.5;
        atomic_store(&rec->synthNow, now);
        if (config->synthetic) {
            if (config->screen) {
                NSDictionary *attrs = @{
                    (id)kCVPixelBufferPixelFormatTypeKey : @(kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange),
                    (id)kCVPixelBufferWidthKey : @(config->width),
                    (id)kCVPixelBufferHeightKey : @(config->height),
                    (id)kCVPixelBufferIOSurfacePropertiesKey : @{},
                };
                CVPixelBufferPoolCreate(NULL, NULL, (__bridge CFDictionaryRef)attrs, &rec->synthPool);
            }
            if (config->camera) {
                NSDictionary *attrs = @{
                    (id)kCVPixelBufferPixelFormatTypeKey : @(kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange),
                    (id)kCVPixelBufferWidthKey : @(config->camera_width),
                    (id)kCVPixelBufferHeightKey : @(config->camera_height),
                    (id)kCVPixelBufferIOSurfacePropertiesKey : @{},
                };
                CVPixelBufferPoolCreate(NULL, NULL, (__bridge CFDictionaryRef)attrs, &rec->synthCameraPool);
            }
            rec->synthDone = dispatch_semaphore_create(0);
            NSThread *thread = [[NSThread alloc] initWithBlock:^{ [rec runSynthetic]; }];
            thread.qualityOfService = NSQualityOfServiceUserInteractive;
            [thread start];
        } else {
            if (config->mic && ![rec startMic:&message]) {
                [rec stopSources];
                setError(error, error_len, message);
                return NULL;
            }
            if (config->camera && ![rec startCamera:&message]) {
                [rec stopSources];
                setError(error, error_len, message);
                return NULL;
            }
            if (config->screen && ![rec startScreen:&message]) {
                [rec stopSources];
                setError(error, error_len, message);
                return NULL;
            }
        }
        if (!config->synthetic_fast && (config->screen || config->camera)) {
            // The pacer writes each slot once its frame had time to arrive.
            rec->timer = dispatch_source_create(DISPATCH_SOURCE_TYPE_TIMER, 0, 0, rec->q);
            uint64_t interval = (uint64_t)(NSEC_PER_SEC / (2 * config->fps));
            dispatch_source_set_timer(rec->timer, dispatch_time(DISPATCH_TIME_NOW, 0), interval, interval / 10);
            __weak KRRecorder *weak = rec;
            dispatch_source_set_event_handler(rec->timer, ^{
                KRRecorder *r = weak;
                if (r) [r emitUpTo:[r now] - kAllowance];
            });
            dispatch_resume(rec->timer);
        }
        // What the OS still holds in memory reaches the disk every 2 s (power loss).
        rec->syncTimer = dispatch_source_create(DISPATCH_SOURCE_TYPE_TIMER, 0, 0, dispatch_get_global_queue(QOS_CLASS_UTILITY, 0));
        dispatch_source_set_timer(rec->syncTimer, dispatch_time(DISPATCH_TIME_NOW, 2 * NSEC_PER_SEC), 2 * NSEC_PER_SEC, NSEC_PER_SEC / 4);
        KRWriter *sw = rec->screenW, *cw = rec->cameraW, *mw = rec->micW;
        dispatch_source_set_event_handler(rec->syncTimer, ^{
            [sw sync];
            [cw sync];
            [mw sync];
        });
        dispatch_resume(rec->syncTimer);
        return (void *)CFBridgingRetain(rec);
    }
}

void kr_rec_pause(void *p) {
    KRRecorder *rec = (__bridge KRRecorder *)p;
    dispatch_sync(rec->q, ^{
        double now = [rec now];
        if (rec->pauseStart < 0 && now > rec->t0) rec->pauseStart = now;
    });
}

void kr_rec_resume(void *p) {
    KRRecorder *rec = (__bridge KRRecorder *)p;
    dispatch_sync(rec->q, ^{
        if (rec->pauseStart < 0) return;
        double now = [rec now];
        [rec->pauses addObject:@[ @(rec->pauseStart), @(now) ]];
        rec->pausedTotal += now - rec->pauseStart;
        rec->pauseStart = -1;
    });
}

static void fillStatus(KRRecorder *rec, KrRecStatus *status) {
    *status = rec->st;
    double end = rec->pauseStart >= 0 ? rec->pauseStart : MIN([rec now], rec->stopAt);
    status->elapsed = MAX(0.0, end - rec->t0 - rec->pausedTotal);
    status->paused = rec->pauseStart >= 0;
    status->mic_peak = MAX(rec->peakNow, rec->peakPrev);
    KRPacer *s = rec->screenP, *c = rec->cameraP;
    status->frames = s ? s->written : 0;
    status->frames_new = s ? s->fresh : 0;
    status->frames_repeated = s ? s->repeated : 0;
    status->frames_late = s ? s->late : 0;
    status->frames_dropped = rec->screenW ? atomic_load(&rec->screenW->dropped) : 0;
    status->camera_frames = c ? c->written : 0;
    status->camera_frames_new = c ? c->fresh : 0;
    status->camera_frames_repeated = c ? c->repeated : 0;
    status->camera_frames_late = c ? c->late : 0;
    status->camera_frames_lost = c ? c->lost : 0;
    status->camera_frames_dropped = rec->cameraW ? atomic_load(&rec->cameraW->dropped) : 0;
    status->audio_dropped = rec->st.audio_dropped + (rec->micW ? atomic_load(&rec->micW->dropped) : 0);
    if (rec->screenW.failure || rec->cameraW.failure || rec->micW.failure) status->failed = 1;
}

void kr_rec_status(void *p, KrRecStatus *status) {
    KRRecorder *rec = (__bridge KRRecorder *)p;
    dispatch_sync(rec->q, ^{ fillStatus(rec, status); });
}

int kr_rec_stop(void *p, KrRecStatus *status, char *error, size_t error_len) {
    @autoreleasepool {
        KRRecorder *rec = CFBridgingRelease(p);
        dispatch_sync(rec->q, ^{
            double now = [rec now];
            rec->stopAt = MAX(rec->t0, rec->pauseStart >= 0 ? rec->pauseStart : now);
        });
        // Late frames and microphone buffers still arrive for a moment.
        if (!rec->cfg.synthetic_fast) usleep(250000);
        [rec stopSources];
        dispatch_sync(rec->q, ^{ [rec emitUpTo:INFINITY]; });
        dispatch_group_t done = dispatch_group_create();
        [rec->screenW finish:done];
        [rec->cameraW finish:done];
        [rec->micW finish:done];
        dispatch_group_wait(done, dispatch_time(DISPATCH_TIME_NOW, 30 * NSEC_PER_SEC));
        __block int result = 0;
        dispatch_sync(rec->q, ^{
            fillStatus(rec, status);
            NSString *failure = rec->error ?: rec->screenW.failure ?: rec->cameraW.failure ?: rec->micW.failure;
            if (failure) {
                setError(error, error_len, failure);
                result = -1;
            }
        });
        return result;
    }
}

#pragma mark - Permissions and devices

int kr_screen_access(void) { return CGPreflightScreenCaptureAccess() ? 1 : 0; }

void kr_request_screen_access(void) { CGRequestScreenCaptureAccess(); }

int kr_mic_access(void) {
    switch ([AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeAudio]) {
    case AVAuthorizationStatusAuthorized: return 3;
    case AVAuthorizationStatusRestricted: return 2;
    case AVAuthorizationStatusDenied: return 1;
    default: return 0;
    }
}

int kr_request_mic_access(void) {
    if ([AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeAudio] == AVAuthorizationStatusNotDetermined) {
        dispatch_semaphore_t answered = dispatch_semaphore_create(0);
        [AVCaptureDevice requestAccessForMediaType:AVMediaTypeAudio completionHandler:^(BOOL granted) { dispatch_semaphore_signal(answered); }];
        dispatch_semaphore_wait(answered, DISPATCH_TIME_FOREVER);
    }
    return kr_mic_access();
}

int kr_camera_access(void) {
    switch ([AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeVideo]) {
    case AVAuthorizationStatusAuthorized: return 3;
    case AVAuthorizationStatusRestricted: return 2;
    case AVAuthorizationStatusDenied: return 1;
    default: return 0;
    }
}

int kr_request_camera_access(void) {
    if ([AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeVideo] == AVAuthorizationStatusNotDetermined) {
        dispatch_semaphore_t answered = dispatch_semaphore_create(0);
        [AVCaptureDevice requestAccessForMediaType:AVMediaTypeVideo completionHandler:^(BOOL granted) { dispatch_semaphore_signal(answered); }];
        dispatch_semaphore_wait(answered, DISPATCH_TIME_FOREVER);
    }
    return kr_camera_access();
}

static char *jsonCopy(id object) {
    NSData *data = [NSJSONSerialization dataWithJSONObject:object options:0 error:nil];
    char *s = calloc(data.length + 1, 1);
    memcpy(s, data.bytes, data.length);
    return s;
}

char *kr_displays_json(void) {
    @autoreleasepool {
        CGDirectDisplayID ids[16];
        uint32_t count = 0;
        CGGetActiveDisplayList(16, ids, &count);
        NSMutableArray *list = [NSMutableArray array];
        int external = 0;
        for (uint32_t i = 0; i < count; i++) {
            if (CGDisplayMirrorsDisplay(ids[i]) != kCGNullDirectDisplay) continue;
            CGDisplayModeRef mode = CGDisplayCopyDisplayMode(ids[i]);
            size_t w = mode ? CGDisplayModeGetPixelWidth(mode) : CGDisplayPixelsWide(ids[i]);
            size_t h = mode ? CGDisplayModeGetPixelHeight(mode) : CGDisplayPixelsHigh(ids[i]);
            if (mode) CGDisplayModeRelease(mode);
            NSString *name = CGDisplayIsBuiltin(ids[i]) ? @"Built-in display" : [NSString stringWithFormat:@"Display %d", ++external];
            [list addObject:@{@"id" : @(ids[i]), @"name" : name, @"width" : @(w), @"height" : @(h), @"main" : CGDisplayIsMain(ids[i]) ? @YES : @NO}];
        }
        return jsonCopy(list);
    }
}

char *kr_mics_json(void) {
    @autoreleasepool {
        AVCaptureDevice *def = [AVCaptureDevice defaultDeviceWithMediaType:AVMediaTypeAudio];
        AVCaptureDeviceDiscoverySession *s = [AVCaptureDeviceDiscoverySession
            discoverySessionWithDeviceTypes:@[ AVCaptureDeviceTypeBuiltInMicrophone, AVCaptureDeviceTypeExternalUnknown ]
                                  mediaType:AVMediaTypeAudio
                                   position:AVCaptureDevicePositionUnspecified];
        NSMutableArray *list = [NSMutableArray array];
        for (AVCaptureDevice *d in s.devices)
            [list addObject:@{@"id" : d.uniqueID, @"name" : d.localizedName, @"default" : [d.uniqueID isEqual:def.uniqueID] ? @YES : @NO}];
        return jsonCopy(list);
    }
}

char *kr_cameras_json(void) {
    @autoreleasepool {
        AVCaptureDevice *def = [AVCaptureDevice defaultDeviceWithMediaType:AVMediaTypeVideo];
        AVCaptureDeviceDiscoverySession *s = [AVCaptureDeviceDiscoverySession
            discoverySessionWithDeviceTypes:@[ AVCaptureDeviceTypeBuiltInWideAngleCamera, AVCaptureDeviceTypeExternalUnknown ]
                                  mediaType:AVMediaTypeVideo
                                   position:AVCaptureDevicePositionUnspecified];
        NSMutableArray *list = [NSMutableArray array];
        for (AVCaptureDevice *d in s.devices) {
            AVCaptureDeviceFormat *f = cameraFormat(d);
            if (!f) continue; // no 30 fps format: not usable for recording
            CMVideoDimensions dim = CMVideoFormatDescriptionGetDimensions(f.formatDescription);
            [list addObject:@{
                @"id" : d.uniqueID, @"name" : d.localizedName, @"default" : [d.uniqueID isEqual:def.uniqueID] ? @YES : @NO,
                @"width" : @(dim.width), @"height" : @(dim.height)
            }];
        }
        return jsonCopy(list);
    }
}

void kr_free(char *s) { free(s); }

#pragma mark - Level meter

@interface KRMonitor : NSObject <AVCaptureAudioDataOutputSampleBufferDelegate> {
  @public
    AVCaptureSession *session;
    _Atomic float peak, peakPrev;
    int64_t count;
}
@end

@implementation KRMonitor
- (void)captureOutput:(AVCaptureOutput *)output didOutputSampleBuffer:(CMSampleBufferRef)sb fromConnection:(AVCaptureConnection *)connection {
    withSamples(sb, ^(const float *s, int64_t n) {
        float p = atomic_load(&self->peak);
        for (int64_t i = 0; i < n; i++) {
            p = MAX(p, fabsf(s[i]));
            if (++self->count >= kRate / 10) {
                atomic_store(&self->peakPrev, p);
                p = 0;
                self->count = 0;
            }
        }
        atomic_store(&self->peak, p);
    });
}
@end

static KRMonitor *gMonitor;
static pthread_mutex_t gMonitorLock = PTHREAD_MUTEX_INITIALIZER;

int kr_mic_monitor_start(const char *mic_id, char *error, size_t error_len) {
    kr_mic_monitor_stop();
    @autoreleasepool {
        AVCaptureDevice *dev = micDevice(mic_id);
        NSError *e = nil;
        AVCaptureDeviceInput *input = dev ? [AVCaptureDeviceInput deviceInputWithDevice:dev error:&e] : nil;
        if (!input) {
            setError(error, error_len, dev ? e.localizedDescription : @"No microphone found. Connect one, or record without the microphone.");
            return -1;
        }
        KRMonitor *m = [KRMonitor new];
        m->session = [AVCaptureSession new];
        [m->session addInput:input];
        AVCaptureAudioDataOutput *out = [AVCaptureAudioDataOutput new];
        out.audioSettings = floatMonoSettings();
        [out setSampleBufferDelegate:m queue:dispatch_queue_create("kreokit.rec.level", DISPATCH_QUEUE_SERIAL)];
        [m->session addOutput:out];
        [m->session startRunning];
        pthread_mutex_lock(&gMonitorLock);
        gMonitor = m;
        pthread_mutex_unlock(&gMonitorLock);
        return 0;
    }
}

float kr_mic_monitor_peak(void) {
    pthread_mutex_lock(&gMonitorLock);
    KRMonitor *m = gMonitor;
    pthread_mutex_unlock(&gMonitorLock);
    return m ? MAX(atomic_load(&m->peak), atomic_load(&m->peakPrev)) : 0;
}

void kr_mic_monitor_stop(void) {
    pthread_mutex_lock(&gMonitorLock);
    KRMonitor *m = gMonitor;
    gMonitor = nil;
    pthread_mutex_unlock(&gMonitorLock);
    if (m) [m->session stopRunning];
}

#pragma mark - Camera preview

@interface KRPreview : NSObject <AVCaptureVideoDataOutputSampleBufferDelegate> {
  @public
    AVCaptureSession *session;
    dispatch_queue_t queue;
    dispatch_source_t synthTimer;
    CVPixelBufferPoolRef synthPool;
    CIContext *context;
    int64_t count;
    NSData *jpeg; // the newest picture, under gPreviewLock
    uint64_t seq;
}
@end

static pthread_mutex_t gPreviewLock = PTHREAD_MUTEX_INITIALIZER;
static KRPreview *gPreview;

@implementation KRPreview
- (void)take:(CVPixelBufferRef)pb {
    static CGColorSpaceRef srgb;
    if (!srgb) srgb = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
    CIImage *image = [CIImage imageWithCVPixelBuffer:pb];
    NSData *data = [context JPEGRepresentationOfImage:image colorSpace:srgb options:@{(id)kCGImageDestinationLossyCompressionQuality : @0.7}];
    if (!data) return;
    pthread_mutex_lock(&gPreviewLock);
    jpeg = data;
    seq++;
    pthread_mutex_unlock(&gPreviewLock);
}

- (void)captureOutput:(AVCaptureOutput *)output didOutputSampleBuffer:(CMSampleBufferRef)sb fromConnection:(AVCaptureConnection *)connection {
    // Every other picture: about 15 a second is enough to see yourself.
    if (count++ % 2) return;
    CVPixelBufferRef pb = CMSampleBufferGetImageBuffer(sb);
    if (pb) [self take:pb];
}

- (void)dealloc {
    if (synthPool) CVPixelBufferPoolRelease(synthPool);
}
@end

int kr_camera_preview_start(const char *camera_id, int width, int height, int synthetic, char *error, size_t error_len) {
    kr_camera_preview_stop();
    @autoreleasepool {
        KRPreview *p = [KRPreview new];
        p->queue = dispatch_queue_create("kreokit.rec.preview", DISPATCH_QUEUE_SERIAL);
        p->context = [CIContext contextWithOptions:nil];
        NSDictionary *attrs = @{
            (id)kCVPixelBufferPixelFormatTypeKey : @(kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange),
            (id)kCVPixelBufferWidthKey : @(width),
            (id)kCVPixelBufferHeightKey : @(height),
        };
        if (synthetic) {
            NSMutableDictionary *pool = [attrs mutableCopy];
            pool[(id)kCVPixelBufferIOSurfacePropertiesKey] = @{};
            CVPixelBufferPoolCreate(NULL, NULL, (__bridge CFDictionaryRef)pool, &p->synthPool);
            p->synthTimer = dispatch_source_create(DISPATCH_SOURCE_TYPE_TIMER, 0, 0, p->queue);
            dispatch_source_set_timer(p->synthTimer, dispatch_time(DISPATCH_TIME_NOW, 0), NSEC_PER_SEC / 15, NSEC_PER_SEC / 100);
            __weak KRPreview *weak = p;
            dispatch_source_set_event_handler(p->synthTimer, ^{
                KRPreview *me = weak;
                CVPixelBufferRef pb = NULL;
                if (!me || CVPixelBufferPoolCreatePixelBuffer(NULL, me->synthPool, &pb) != kCVReturnSuccess) return;
                int64_t n = me->count++;
                drawPattern(pb, n, n % 15 == 0);
                [me take:pb];
                CVPixelBufferRelease(pb);
            });
            dispatch_resume(p->synthTimer);
        } else {
            AVCaptureDevice *dev = cameraDevice(camera_id);
            NSError *e = nil;
            AVCaptureDeviceInput *input = dev ? [AVCaptureDeviceInput deviceInputWithDevice:dev error:&e] : nil;
            if (!input) {
                setError(error, error_len, dev ? [NSString stringWithFormat:@"The camera can't be opened (%@). Check that KreoKit Rec may use it in System Preferences > Security & Privacy > Privacy > Camera, and that no other app is using it.", e.localizedDescription]
                                               : @"No camera found. Connect one, or record without the camera.");
                return -1;
            }
            p->session = [AVCaptureSession new];
            [p->session beginConfiguration];
            [p->session addInput:input];
            AVCaptureVideoDataOutput *out = [AVCaptureVideoDataOutput new];
            out.videoSettings = attrs;
            [out setSampleBufferDelegate:p queue:p->queue];
            [p->session addOutput:out];
            [p->session commitConfiguration];
            [p->session startRunning];
            if (!p->session.running) {
                setError(error, error_len, @"The camera didn't start. Check that no other app is using it, then choose it again.");
                return -1;
            }
        }
        pthread_mutex_lock(&gPreviewLock);
        gPreview = p;
        pthread_mutex_unlock(&gPreviewLock);
        return 0;
    }
}

size_t kr_camera_preview_frame(uint64_t *seq, char **jpeg) {
    *jpeg = NULL;
    pthread_mutex_lock(&gPreviewLock);
    NSData *data = gPreview && gPreview->seq > *seq ? gPreview->jpeg : nil;
    if (data) *seq = gPreview->seq;
    pthread_mutex_unlock(&gPreviewLock);
    if (!data.length) return 0;
    *jpeg = malloc(data.length);
    memcpy(*jpeg, data.bytes, data.length);
    return data.length;
}

void kr_camera_preview_stop(void) {
    pthread_mutex_lock(&gPreviewLock);
    KRPreview *p = gPreview;
    gPreview = nil;
    pthread_mutex_unlock(&gPreviewLock);
    if (!p) return;
    if (p->synthTimer) dispatch_source_cancel(p->synthTimer);
    [p->session stopRunning];
    // Waits for a picture still being made.
    dispatch_sync(p->queue, ^{});
}
