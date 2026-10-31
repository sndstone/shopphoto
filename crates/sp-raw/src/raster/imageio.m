#import <Foundation/Foundation.h>
#import <ImageIO/ImageIO.h>
#import <CoreGraphics/CoreGraphics.h>

// All returned allocations belong to the caller until sp_imageio_free.
// Integer RGBA is native-endian, unassociated only when alpha is fully opaque.
typedef struct {
    uint16_t *pixels;
    size_t width, height;
    uint32_t orientation;
    uint8_t *icc;
    size_t icc_len;
    uint8_t *metadata;
    size_t metadata_len;
} SpImageIO;

void sp_imageio_free(SpImageIO *out) {
    free(out->pixels);
    free(out->icc);
    free(out->metadata);
    memset(out, 0, sizeof(*out));
}

static void copy_data(NSData *data, uint8_t **bytes, size_t *length) {
    if (data.length) {
        *bytes = malloc(data.length);
        if (*bytes) { memcpy(*bytes, data.bytes, data.length); *length = data.length; }
    }
}

int sp_imageio_decode(const char *path, int thumbnail, SpImageIO *out,
                      char *error, size_t error_len) {
    @autoreleasepool {
        CGImageSourceRef source = NULL;
        CGImageRef image = NULL;
        CGContextRef context = NULL;
        const char *failure = "ImageIO could not open this photograph";
        NSDictionary *properties = nil, *exif = nil, *tiff = nil, *tags = nil;
        NSMutableDictionary *meta = nil;
        NSData *icc = nil;
        NSArray *iso = nil;
        NSString *date = nil;
        memset(out, 0, sizeof(*out));
        NSURL *url = [NSURL fileURLWithFileSystemRepresentation:path isDirectory:NO relativeToURL:nil];
        source = CGImageSourceCreateWithURL((__bridge CFURLRef)url,
            (__bridge CFDictionaryRef)@{(__bridge NSString *)kCGImageSourceShouldCache: @NO});
        if (!source) goto done;
        size_t index = CGImageSourceGetPrimaryImageIndex(source);
        properties = CFBridgingRelease(CGImageSourceCopyPropertiesAtIndex(source, index, NULL));
        uint64_t width = [properties[(__bridge NSString *)kCGImagePropertyPixelWidth] unsignedLongLongValue];
        uint64_t height = [properties[(__bridge NSString *)kCGImagePropertyPixelHeight] unsignedLongLongValue];
        // Validate dimensions before asking ImageIO to materialize any pixels.
        failure = "Raster source exceeds the 150 MP / 2.5 GB memory limit or has invalid dimensions";
        if (!width || !height || width > 150000000 || height > 150000000 / width ||
            width * height > 2500000000ULL / 8) goto done;
        out->orientation = [properties[(__bridge NSString *)kCGImagePropertyOrientation] unsignedIntValue];
        if (thumbnail) {
            image = CGImageSourceCreateThumbnailAtIndex(source, index, (__bridge CFDictionaryRef)@{
                (__bridge NSString *)kCGImageSourceCreateThumbnailFromImageAlways: @YES,
                (__bridge NSString *)kCGImageSourceCreateThumbnailWithTransform: @YES,
                (__bridge NSString *)kCGImageSourceThumbnailMaxPixelSize: @512});
            out->orientation = 1;
        } else {
            image = CGImageSourceCreateImageAtIndex(source, index, (__bridge CFDictionaryRef)@{
                (__bridge NSString *)kCGImageSourceShouldCacheImmediately: @YES});
        }
        failure = "ImageIO could not decode this photograph (codec availability depends on macOS version)";
        if (!image) goto done;
        width = CGImageGetWidth(image); height = CGImageGetHeight(image);
        if (!width || !height || width > 150000000 || height > 150000000 / width) goto done;
        CGColorSpaceRef space = CGImageGetColorSpace(image);
        failure = "ImageIO photograph needs an RGB color space with an ICC profile";
        if (!space || CGColorSpaceGetModel(space) != kCGColorSpaceModelRGB) goto done;
        icc = CFBridgingRelease(CGColorSpaceCopyICCData(space));
        if (!icc.length) goto done;
        copy_data(icc, &out->icc, &out->icc_len);
        failure = "ImageIO could not allocate the 16-bit RGB bitmap";
        out->pixels = calloc((size_t)(width * height), 8);
        if (!out->pixels || !out->icc) goto done;
        context = CGBitmapContextCreate(out->pixels, width, height, 16, width * 8, space,
            kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder16Host);
        if (!context) goto done;
        CGContextSetBlendMode(context, kCGBlendModeCopy);
        CGContextDrawImage(context, CGRectMake(0, 0, width, height), image);
        out->width = width; out->height = height;
        exif = properties[(__bridge NSString *)kCGImagePropertyExifDictionary];
        tiff = properties[(__bridge NSString *)kCGImagePropertyTIFFDictionary];
        meta = [NSMutableDictionary dictionary];
        meta[@"camera_make"] = tiff[(__bridge NSString *)kCGImagePropertyTIFFMake] ?: @"";
        meta[@"camera_model"] = tiff[(__bridge NSString *)kCGImagePropertyTIFFModel] ?: @"";
        tags = @{@"shutter_s": (__bridge NSString *)kCGImagePropertyExifExposureTime,
            @"aperture_f": (__bridge NSString *)kCGImagePropertyExifFNumber,
            @"focal_length_mm": (__bridge NSString *)kCGImagePropertyExifFocalLength,
            @"lens_model": (__bridge NSString *)kCGImagePropertyExifLensModel};
        for (NSString *key in tags) if (exif[tags[key]]) meta[key] = exif[tags[key]];
        iso = exif[(__bridge NSString *)kCGImagePropertyExifISOSpeedRatings];
        if (iso.count) meta[@"iso"] = iso[0];
        date = exif[(__bridge NSString *)kCGImagePropertyExifDateTimeOriginal];
        if (date.length == 19) {
            NSMutableString *normalized = [date mutableCopy];
            [normalized replaceCharactersInRange:NSMakeRange(4, 1) withString:@"-"];
            [normalized replaceCharactersInRange:NSMakeRange(7, 1) withString:@"-"];
            [normalized replaceCharactersInRange:NSMakeRange(10, 1) withString:@"T"];
            meta[@"captured_at"] = normalized;
        }
        copy_data([NSJSONSerialization dataWithJSONObject:meta options:0 error:nil],
            &out->metadata, &out->metadata_len);
        failure = NULL;
    done:
        if (context) CGContextRelease(context);
        if (image) CGImageRelease(image);
        if (source) CFRelease(source);
        if (failure) {
            if (error_len) snprintf(error, error_len, "%s", failure);
            sp_imageio_free(out);
            return 0;
        }
        return 1;
    }
}

// Small asymmetric, tagged fixture for Rust integration tests. Orientation is
// stored as metadata, so the full-resolution path must apply it exactly once.
int sp_imageio_test_image(const char *path, uint32_t orientation, int heic, int p3) {
    @autoreleasepool {
        uint8_t pixels[128 * 64 * 4];
        for (size_t y = 0; y < 64; ++y) for (size_t x = 0; x < 128; ++x) {
            size_t p = (y * 128 + x) * 4;
            pixels[p] = x < 64 ? 200 : 40;
            pixels[p+1] = y < 32 ? 100 : 160; pixels[p+2] = x < 64 ? 40 : 200; pixels[p+3] = 255;
        }
        CGColorSpaceRef space = CGColorSpaceCreateWithName(p3 ? kCGColorSpaceDisplayP3 : kCGColorSpaceSRGB);
        CGContextRef context = CGBitmapContextCreate(pixels, 128, 64, 8, 512, space,
            kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big);
        CGImageRef image = context ? CGBitmapContextCreateImage(context) : NULL;
        NSURL *url = [NSURL fileURLWithFileSystemRepresentation:path isDirectory:NO relativeToURL:nil];
        CGImageDestinationRef dest = CGImageDestinationCreateWithURL((__bridge CFURLRef)url, heic ? CFSTR("public.heic") : CFSTR("public.png"), 1, NULL);
        BOOL ok = NO;
        if (dest && image) {
            CGImageDestinationAddImage(dest, image, (__bridge CFDictionaryRef)@{
                (__bridge NSString *)kCGImagePropertyOrientation: @(orientation),
                (__bridge NSString *)kCGImageDestinationLossyCompressionQuality: @1.0,
                (__bridge NSString *)kCGImagePropertyTIFFDictionary: @{
                    (__bridge NSString *)kCGImagePropertyTIFFMake: @"ShopPhoto Test",
                    (__bridge NSString *)kCGImagePropertyTIFFModel: @"ImageIO fixture"},
                (__bridge NSString *)kCGImagePropertyExifDictionary: @{
                    (__bridge NSString *)kCGImagePropertyExifISOSpeedRatings: @[@200],
                    (__bridge NSString *)kCGImagePropertyExifExposureTime: @(1.0 / 125.0),
                    (__bridge NSString *)kCGImagePropertyExifFNumber: @2.8,
                    (__bridge NSString *)kCGImagePropertyExifFocalLength: @35,
                    (__bridge NSString *)kCGImagePropertyExifDateTimeOriginal: @"2026:10:22 12:34:56"}});
            ok = CGImageDestinationFinalize(dest);
        }
        if (dest) CFRelease(dest);
        if (image) CGImageRelease(image);
        if (context) CGContextRelease(context);
        CGColorSpaceRelease(space);
        return ok;
    }
}
