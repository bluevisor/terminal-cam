// CoreMediaIO sink-stream producer for the Terminal Cam virtual camera.
//
// Connects to the CMIO device published by the camera extension
// (macos/Extension), finds its sink stream, and enqueues BGRA frames.
// The extension forwards them to consuming apps via its source stream.
//
// Called from Rust (src/vcam.rs). All functions are thread-compatible but
// not thread-safe; callers must serialize access to a handle.

#import <CoreFoundation/CoreFoundation.h>
#import <CoreMedia/CoreMedia.h>
#import <CoreMediaIO/CMIOHardware.h>
#import <CoreVideo/CoreVideo.h>

#include <stdbool.h>
#include <stdint.h>
#include <string.h>

typedef struct {
    CMIODeviceID device;
    CMIOStreamID stream;
    CMSimpleQueueRef queue;
    CMFormatDescriptionRef format;
    CVPixelBufferPoolRef pool;
    int32_t width;
    int32_t height;
    bool stream_started;
} VCam;

static void vcam_queue_altered(CMIOStreamID stream, void *token, void *refcon) {
    (void)stream;
    (void)token;
    (void)refcon;
}

static bool vcam_find_device(CFStringRef uid, CMIODeviceID *out_device) {
    CMIOObjectPropertyAddress address = {
        .mSelector = kCMIOHardwarePropertyDevices,
        .mScope = kCMIOObjectPropertyScopeGlobal,
        .mElement = kCMIOObjectPropertyElementMain,
    };

    UInt32 size = 0;
    if (CMIOObjectGetPropertyDataSize(kCMIOObjectSystemObject, &address, 0, NULL, &size) != noErr ||
        size == 0) {
        return false;
    }

    UInt32 count = size / sizeof(CMIOObjectID);
    CMIOObjectID *devices = calloc(count, sizeof(CMIOObjectID));
    if (!devices) {
        return false;
    }

    UInt32 used = 0;
    if (CMIOObjectGetPropertyData(kCMIOObjectSystemObject, &address, 0, NULL, size, &used,
                                  devices) != noErr) {
        free(devices);
        return false;
    }

    bool found = false;
    for (UInt32 i = 0; i < count && !found; i++) {
        CMIOObjectPropertyAddress uid_address = {
            .mSelector = kCMIODevicePropertyDeviceUID,
            .mScope = kCMIOObjectPropertyScopeGlobal,
            .mElement = kCMIOObjectPropertyElementMain,
        };
        CFStringRef device_uid = NULL;
        UInt32 uid_size = sizeof(device_uid);
        if (CMIOObjectGetPropertyData(devices[i], &uid_address, 0, NULL, uid_size, &used,
                                      &device_uid) != noErr ||
            device_uid == NULL) {
            continue;
        }
        if (CFStringCompare(device_uid, uid, kCFCompareCaseInsensitive) == kCFCompareEqualTo) {
            *out_device = devices[i];
            found = true;
        }
        CFRelease(device_uid);
    }

    free(devices);
    return found;
}

// The extension registers its source stream first and the sink second, so
// the sink is stream index 1.
static bool vcam_find_sink_stream(CMIODeviceID device, CMIOStreamID *out_stream) {
    CMIOObjectPropertyAddress address = {
        .mSelector = kCMIODevicePropertyStreams,
        .mScope = kCMIOObjectPropertyScopeGlobal,
        .mElement = kCMIOObjectPropertyElementMain,
    };

    UInt32 size = 0;
    if (CMIOObjectGetPropertyDataSize(device, &address, 0, NULL, &size) != noErr) {
        return false;
    }
    if (size < 2 * sizeof(CMIOStreamID)) {
        return false;
    }

    UInt32 count = size / sizeof(CMIOStreamID);
    CMIOStreamID *streams = calloc(count, sizeof(CMIOStreamID));
    if (!streams) {
        return false;
    }

    UInt32 used = 0;
    bool ok = CMIOObjectGetPropertyData(device, &address, 0, NULL, size, &used, streams) == noErr;
    if (ok) {
        *out_stream = streams[1];
    }
    free(streams);
    return ok;
}

void *vcam_open(const char *device_uid_utf8, int32_t width, int32_t height) {
    CFStringRef uid =
        CFStringCreateWithCString(kCFAllocatorDefault, device_uid_utf8, kCFStringEncodingUTF8);
    if (!uid) {
        return NULL;
    }

    CMIODeviceID device = 0;
    bool found = vcam_find_device(uid, &device);
    CFRelease(uid);
    if (!found) {
        return NULL;
    }

    CMIOStreamID stream = 0;
    if (!vcam_find_sink_stream(device, &stream)) {
        return NULL;
    }

    VCam *vcam = calloc(1, sizeof(VCam));
    if (!vcam) {
        return NULL;
    }
    vcam->device = device;
    vcam->stream = stream;
    vcam->width = width;
    vcam->height = height;

    if (CMIOStreamCopyBufferQueue(stream, vcam_queue_altered, NULL, &vcam->queue) != noErr ||
        vcam->queue == NULL) {
        free(vcam);
        return NULL;
    }

    if (CMVideoFormatDescriptionCreate(kCFAllocatorDefault, kCVPixelFormatType_32BGRA, width,
                                       height, NULL, &vcam->format) != noErr) {
        CFRelease(vcam->queue);
        free(vcam);
        return NULL;
    }

    NSDictionary *attributes = @{
        (id)kCVPixelBufferPixelFormatTypeKey : @(kCVPixelFormatType_32BGRA),
        (id)kCVPixelBufferWidthKey : @(width),
        (id)kCVPixelBufferHeightKey : @(height),
        (id)kCVPixelBufferIOSurfacePropertiesKey : @{},
    };
    if (CVPixelBufferPoolCreate(kCFAllocatorDefault, NULL, (__bridge CFDictionaryRef)attributes,
                                &vcam->pool) != kCVReturnSuccess) {
        CFRelease(vcam->format);
        CFRelease(vcam->queue);
        free(vcam);
        return NULL;
    }

    if (CMIODeviceStartStream(device, stream) != noErr) {
        CVPixelBufferPoolRelease(vcam->pool);
        CFRelease(vcam->format);
        CFRelease(vcam->queue);
        free(vcam);
        return NULL;
    }
    vcam->stream_started = true;

    return vcam;
}

// Pushes one tightly-packed BGRA frame (width * height * 4 bytes).
// Returns 0 on success, 1 if the frame was dropped (queue full / pool
// exhausted — normal backpressure), and -1 on hard failure (caller should
// close and reconnect).
int32_t vcam_push(void *handle, const uint8_t *bgra) {
    VCam *vcam = (VCam *)handle;
    if (!vcam || !bgra) {
        return -1;
    }

    if (CMSimpleQueueGetCount(vcam->queue) >= CMSimpleQueueGetCapacity(vcam->queue)) {
        return 1;
    }

    NSDictionary *aux = @{(id)kCVPixelBufferPoolAllocationThresholdKey : @(8)};
    CVPixelBufferRef pixel_buffer = NULL;
    CVReturn cv_status = CVPixelBufferPoolCreatePixelBufferWithAuxAttributes(
        kCFAllocatorDefault, vcam->pool, (__bridge CFDictionaryRef)aux, &pixel_buffer);
    if (cv_status == kCVReturnWouldExceedAllocationThreshold) {
        return 1;
    }
    if (cv_status != kCVReturnSuccess || pixel_buffer == NULL) {
        return -1;
    }

    CVPixelBufferLockBaseAddress(pixel_buffer, 0);
    uint8_t *dst = CVPixelBufferGetBaseAddress(pixel_buffer);
    size_t dst_stride = CVPixelBufferGetBytesPerRow(pixel_buffer);
    size_t src_stride = (size_t)vcam->width * 4;
    if (dst_stride == src_stride) {
        memcpy(dst, bgra, src_stride * (size_t)vcam->height);
    } else {
        for (int32_t row = 0; row < vcam->height; row++) {
            memcpy(dst + (size_t)row * dst_stride, bgra + (size_t)row * src_stride, src_stride);
        }
    }
    CVPixelBufferUnlockBaseAddress(pixel_buffer, 0);

    CMSampleTimingInfo timing = {
        .duration = kCMTimeInvalid,
        .presentationTimeStamp = CMClockGetTime(CMClockGetHostTimeClock()),
        .decodeTimeStamp = kCMTimeInvalid,
    };
    CMSampleBufferRef sample_buffer = NULL;
    OSStatus status =
        CMSampleBufferCreateForImageBuffer(kCFAllocatorDefault, pixel_buffer, true, NULL, NULL,
                                           vcam->format, &timing, &sample_buffer);
    CVPixelBufferRelease(pixel_buffer);
    if (status != noErr || sample_buffer == NULL) {
        return -1;
    }

    // On success the queue consumer takes ownership of the retained sample
    // buffer; only release it ourselves if the enqueue failed.
    if (CMSimpleQueueEnqueue(vcam->queue, sample_buffer) != noErr) {
        CFRelease(sample_buffer);
        return 1;
    }

    return 0;
}

void vcam_close(void *handle) {
    VCam *vcam = (VCam *)handle;
    if (!vcam) {
        return;
    }
    if (vcam->stream_started) {
        CMIODeviceStopStream(vcam->device, vcam->stream);
    }
    if (vcam->pool) {
        CVPixelBufferPoolRelease(vcam->pool);
    }
    if (vcam->format) {
        CFRelease(vcam->format);
    }
    if (vcam->queue) {
        CFRelease(vcam->queue);
    }
    free(vcam);
}
