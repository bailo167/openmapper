// SPDX-License-Identifier: Apache-2.0
// C interface between om-syphon (Rust) and the vendored Syphon framework.
// Frames cross the CPU: BGRA8 bytes in, BGRA8 bytes out.

#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#import <Syphon/SyphonMetalServer.h>
#import <Syphon/SyphonMetalClient.h>
#import <Syphon/SyphonServerDirectory.h>
#include <stdint.h>

@interface OMSyphonServer : NSObject
@property(strong) id<MTLDevice> device;
@property(strong) id<MTLCommandQueue> queue;
@property(strong) SyphonMetalServer *server;
@property(strong) id<MTLTexture> texture;
@end
@implementation OMSyphonServer
@end

@interface OMSyphonClient : NSObject
@property(strong) id<MTLDevice> device;
@property(strong) id<MTLCommandQueue> queue;
@property(strong) SyphonMetalClient *client;
@property(strong) id<MTLBuffer> buffer;
@end
@implementation OMSyphonClient
@end

// Largest frame accepted from another application (8192 x 8192).
static const NSUInteger OM_MAX_PIXELS = 8192u * 8192u;

// Announcement dictionaries come from other processes (distributed
// notifications) and may hold any property-list type: accept strings only.
static const char *om_utf8(id value) {
    if (![value isKindOfClass:[NSString class]]) return "";
    const char *s = [(NSString *)value UTF8String];
    return s ? s : "";
}

// Objective-C exceptions must never unwind into Rust: every exported
// function catches them and returns its failure value. OpenMapper patch
// policy: no exception crosses the C interface.

static NSString *om_string(const char *s) {
    if (s == NULL || s[0] == 0) return nil;
    return [NSString stringWithUTF8String:s];
}

void *om_syphon_server_new(const char *name) {
    @autoreleasepool {
        @try {
            id<MTLDevice> device = MTLCreateSystemDefaultDevice();
            if (device == nil) return NULL;
            OMSyphonServer *s = [OMSyphonServer new];
            s.device = device;
            s.queue = [device newCommandQueue];
            s.server = [[SyphonMetalServer alloc] initWithName:om_string(name) device:device options:nil];
            if (s.queue == nil || s.server == nil) return NULL;
            return (void *)CFBridgingRetain(s);
        } @catch (NSException *e) {
            return NULL;
        }
    }
}

/// Publishes one BGRA8 frame (rows top to bottom). 0 on success.
int om_syphon_server_publish(void *handle, const uint8_t *bgra, uint32_t w, uint32_t h) {
    @autoreleasepool {
        @try {
            if (handle == NULL || bgra == NULL || w == 0 || h == 0) return -1;
            OMSyphonServer *s = (__bridge OMSyphonServer *)handle;
            if (s.texture == nil || s.texture.width != w || s.texture.height != h) {
                MTLTextureDescriptor *d =
                    [MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatBGRA8Unorm
                                                                       width:w
                                                                      height:h
                                                                   mipmapped:NO];
                d.usage = MTLTextureUsageShaderRead;
                d.storageMode = MTLStorageModeManaged;
                s.texture = [s.device newTextureWithDescriptor:d];
                if (s.texture == nil) return -2;
            }
            [s.texture replaceRegion:MTLRegionMake2D(0, 0, w, h)
                         mipmapLevel:0
                           withBytes:bgra
                         bytesPerRow:(NSUInteger)w * 4];
            id<MTLCommandBuffer> cmd = [s.queue commandBuffer];
            if (cmd == nil) return -3;
            [s.server publishFrameTexture:s.texture
                          onCommandBuffer:cmd
                              imageRegion:NSMakeRect(0, 0, w, h)
                                  flipped:NO];
            [cmd commit];
            [cmd waitUntilCompleted];
            return cmd.status == MTLCommandBufferStatusCompleted ? 0 : -4;
        } @catch (NSException *e) {
            return -5;
        }
    }
}

void om_syphon_server_free(void *handle) {
    @autoreleasepool {
        @try {
            if (handle == NULL) return;
            OMSyphonServer *s = (OMSyphonServer *)CFBridgingRelease(handle);
            [s.server stop];
        } @catch (NSException *e) {
            return;
        }
    }
}

/// Runs the calling thread's run loop for `seconds` (distributed
/// notifications, which Syphon uses for discovery, arrive on the main
/// thread's run loop).
void om_syphon_run_loop(double seconds) {
    @autoreleasepool {
        @try {
            CFRunLoopRunInMode(kCFRunLoopDefaultMode, seconds, false);
        } @catch (NSException *e) {
        }
    }
}

int om_syphon_is_main_thread(void) {
    return [NSThread isMainThread] ? 1 : 0;
}

typedef void (*om_syphon_list_cb)(void *ctx, const char *name, const char *app);

/// Reports every announced server.
void om_syphon_list(om_syphon_list_cb cb, void *ctx) {
    @autoreleasepool {
        @try {
            NSArray *servers = [[SyphonServerDirectory sharedDirectory] servers];
            for (NSDictionary *d in servers) {
                if (![d isKindOfClass:[NSDictionary class]]) continue;
                cb(ctx, om_utf8(d[SyphonServerDescriptionNameKey]),
                   om_utf8(d[SyphonServerDescriptionAppNameKey]));
            }
        } @catch (NSException *e) {
            return;
        }
    }
}

/// Connects to the first server matching `name` and `app` (empty matches
/// any). NULL if none is announced.
void *om_syphon_client_new(const char *name, const char *app) {
    @autoreleasepool {
        @try {
            NSArray *matches = [[SyphonServerDirectory sharedDirectory] serversMatchingName:om_string(name)
                                                                                    appName:om_string(app)];
            NSDictionary *desc = matches.firstObject;
            if (desc == nil) return NULL;
            id<MTLDevice> device = MTLCreateSystemDefaultDevice();
            if (device == nil) return NULL;
            OMSyphonClient *c = [OMSyphonClient new];
            c.device = device;
            c.queue = [device newCommandQueue];
            c.client = [[SyphonMetalClient alloc] initWithServerDescription:desc
                                                                    device:device
                                                                   options:nil
                                                           newFrameHandler:nil];
            if (c.queue == nil || c.client == nil) return NULL;
            return (void *)CFBridgingRetain(c);
        } @catch (NSException *e) {
            return NULL;
        }
    }
}

/// 1: a new frame is in `*data` (BGRA8, `*w`×`*h`, tightly packed, valid
/// until the next call); 0: no new frame; -1: the server has gone.
int om_syphon_client_next(void *handle, const uint8_t **data, uint32_t *w, uint32_t *h) {
    @autoreleasepool {
        @try {
            if (handle == NULL) return -1;
            OMSyphonClient *c = (__bridge OMSyphonClient *)handle;
            if (!c.client.isValid) return -1;
            if (!c.client.hasNewFrame) return 0;
            id<MTLTexture> tex = [c.client newFrameImage];
            if (tex == nil) return 0;
            NSUInteger tw = tex.width, th = tex.height;
            if (tw == 0 || th == 0 || tex.pixelFormat != MTLPixelFormatBGRA8Unorm) return 0;
        if (tw > 16384 || th > 16384 || tw * th > OM_MAX_PIXELS) return 0;
            NSUInteger len = tw * th * 4;
            if (c.buffer == nil || c.buffer.length < len) {
                c.buffer = [c.device newBufferWithLength:len options:MTLResourceStorageModeShared];
                if (c.buffer == nil) return 0;
            }
            id<MTLCommandBuffer> cmd = [c.queue commandBuffer];
            id<MTLBlitCommandEncoder> blit = [cmd blitCommandEncoder];
            [blit copyFromTexture:tex
                             sourceSlice:0
                             sourceLevel:0
                            sourceOrigin:MTLOriginMake(0, 0, 0)
                              sourceSize:MTLSizeMake(tw, th, 1)
                                toBuffer:c.buffer
                       destinationOffset:0
                  destinationBytesPerRow:tw * 4
                destinationBytesPerImage:len];
            [blit endEncoding];
            [cmd commit];
            [cmd waitUntilCompleted];
            if (cmd.status != MTLCommandBufferStatusCompleted) return 0;
            *data = (const uint8_t *)c.buffer.contents;
            *w = (uint32_t)tw;
            *h = (uint32_t)th;
            return 1;
        } @catch (NSException *e) {
            return -1;
        }
    }
}

void om_syphon_client_free(void *handle) {
    @autoreleasepool {
        @try {
            if (handle == NULL) return;
            OMSyphonClient *c = (OMSyphonClient *)CFBridgingRelease(handle);
            [c.client stop];
        } @catch (NSException *e) {
            return;
        }
    }
}
