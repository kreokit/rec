// Shared by capbench.m (synthetic sources) and capreal.m (real devices): one fragmented
// MP4 writer per source, the H.264 settings, timing and CPU measurement.

#import <AVFoundation/AVFoundation.h>
#import <CoreMedia/CoreMedia.h>
#import <CoreVideo/CoreVideo.h>
#import <Foundation/Foundation.h>
#include <fcntl.h>
#include <mach/mach.h>
#include <mach/mach_time.h>
#include <sys/resource.h>

static const int kAudioRate = 48000;
static const int kAudioChunk = 480; // 10 ms

// ---------------------------------------------------------------------------------------
// One output file with one track.

@interface Track : NSObject
@property(nonatomic) NSString *name;
@property(nonatomic) AVAssetWriter *writer;
@property(nonatomic) AVAssetWriterInput *input;
@property(nonatomic) AVAssetWriterInputPixelBufferAdaptor *adaptor;
@property(nonatomic) dispatch_queue_t queue;
@property(nonatomic) int64_t produced, appended, droppedNotReady, appendFailed;
@property(nonatomic) double maxAppendMs, sumAppendMs;
@end
@implementation Track
@end

static Track *makeTrack(NSString *dir, NSString *name, NSString *fileType, double fragment,
                        AVMediaType type, NSDictionary *settings) {
    NSString *ext = [fileType isEqual:AVFileTypeQuickTimeMovie] ? @"mov" : @"mp4";
    NSURL *url = [NSURL fileURLWithPath:[dir stringByAppendingFormat:@"/%@.%@", name, ext]];
    [[NSFileManager defaultManager] removeItemAtURL:url error:nil];
    NSError *error = nil;
    Track *t = [Track new];
    t.name = name;
    t.writer = [AVAssetWriter assetWriterWithURL:url fileType:fileType error:&error];
    if (!t.writer) {
        fprintf(stderr, "writer %s: %s\n", name.UTF8String, error.description.UTF8String);
        exit(1);
    }
    if (fragment > 0) t.writer.movieFragmentInterval = CMTimeMakeWithSeconds(fragment, 600);
    t.input = [AVAssetWriterInput assetWriterInputWithMediaType:type outputSettings:settings];
    t.input.expectsMediaDataInRealTime = YES;
    if (![t.writer canAddInput:t.input]) {
        fprintf(stderr, "writer %s refuses the input settings\n", name.UTF8String);
        exit(1);
    }
    [t.writer addInput:t.input];
    t.queue = dispatch_queue_create(name.UTF8String, DISPATCH_QUEUE_SERIAL);
    return t;
}

static double gKeyframeSeconds = 1.0;

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
            AVVideoMaxKeyFrameIntervalKey : @((int)(fps * gKeyframeSeconds)),
            AVVideoMaxKeyFrameIntervalDurationKey : @(gKeyframeSeconds),
            AVVideoAllowFrameReorderingKey : @NO, // no B-frames: lower latency, simpler sync
        },
    };
}

// Opens the file and its encoder before recording starts (in the app: during the 3-2-1
// countdown), so the first frames don't wait for the encoder to warm up.
static void startTrack(Track *t, CMTime pts) {
    if (![t.writer startWriting]) {
        fprintf(stderr, "start %s: %s\n", t.name.UTF8String, t.writer.error.description.UTF8String);
        exit(1);
    }
    [t.writer startSessionAtSourceTime:pts];
}

static double nowMs(void) {
    static mach_timebase_info_data_t tb;
    if (!tb.denom) mach_timebase_info(&tb);
    return (double)mach_absolute_time() * tb.numer / tb.denom / 1e6;
}

static CMTime hostNow(void) { return CMClockGetTime(CMClockGetHostTimeClock()); }

static void appendTimed(Track *t, BOOL (^append)(void)) {
    t.produced++;
    if (!t.input.readyForMoreMediaData) {
        t.droppedNotReady++;
        return;
    }
    double a = nowMs();
    BOOL ok = append();
    double ms = nowMs() - a;
    t.sumAppendMs += ms;
    if (ms > t.maxAppendMs) t.maxAppendMs = ms;
    if (ok) t.appended++;
    else t.appendFailed++;
}

// ---------------------------------------------------------------------------------------
// System-wide CPU (the encoder may run in a system service, not in our process).

static uint64_t cpuTicks(uint64_t *idle) {
    host_cpu_load_info_data_t info;
    mach_msg_type_number_t count = HOST_CPU_LOAD_INFO_COUNT;
    host_statistics(mach_host_self(), HOST_CPU_LOAD_INFO, (host_info_t)&info, &count);
    uint64_t total = 0;
    for (int i = 0; i < CPU_STATE_MAX; i++) total += info.cpu_ticks[i];
    *idle = info.cpu_ticks[CPU_STATE_IDLE];
    return total;
}

static double processCpuSeconds(void) {
    struct rusage u;
    getrusage(RUSAGE_SELF, &u);
    return u.ru_utime.tv_sec + u.ru_utime.tv_usec / 1e6 + u.ru_stime.tv_sec + u.ru_stime.tv_usec / 1e6;
}

static BOOL parseSize(const char *s, int *w, int *h) { return sscanf(s, "%dx%d", w, h) == 2; }
