#import <CoreML/CoreML.h>

void *sp_ai_model_open(const char *path, int compute, char *error, size_t error_len) {
    @autoreleasepool {
        NSString *p = [NSString stringWithUTF8String:path];
        MLModelConfiguration *config = [MLModelConfiguration new];
        // Explicit per-session policy: changing app preferences never mutates
        // process-wide environment or changes an inference already in progress.
        config.computeUnits = MLComputeUnitsAll;
        switch (compute) {
            case 1: config.computeUnits = MLComputeUnitsCPUAndGPU; break;
            case 2:
                if (@available(macOS 13.0, *)) {
                    config.computeUnits = MLComputeUnitsCPUAndNeuralEngine;
                } else {
                    if (error_len) snprintf(error, error_len, "Neural Engine policy requires macOS 13 or newer");
                    return NULL;
                }
                break;
            case 3: config.computeUnits = MLComputeUnitsCPUOnly; break;
            default: break;
        }
        NSError *e = nil;
        MLModel *model = [MLModel modelWithContentsOfURL:[NSURL fileURLWithPath:p]
                                            configuration:config error:&e];
        if (!model && error_len) snprintf(error, error_len, "%s", e.localizedDescription.UTF8String);
        return model ? (__bridge_retained void *)model : NULL;
    }
}

int sp_ai_model_predict(void *handle, const float *input, float *output,
                        char *error, size_t error_len) {
    @autoreleasepool {
        NSError *e = nil;
        MLMultiArray *array = [[MLMultiArray alloc] initWithShape:@[@1,@4,@512,@512]
            dataType:MLMultiArrayDataTypeFloat32 error:&e];
        if (!array) { if (error_len) snprintf(error, error_len, "%s", e.localizedDescription.UTF8String); return 0; }
        memcpy(array.dataPointer, input, 4 * 512 * 512 * sizeof(float));
        // Exports differ in the input name (`inp_1`, `inp`); every denoise
        // model has exactly one input.
        MLModel *model = (__bridge MLModel *)handle;
        NSDictionary<NSString *, MLFeatureDescription *> *inputs =
            model.modelDescription.inputDescriptionsByName;
        NSString *input_name = inputs.count == 1 ? inputs.allKeys.firstObject : @"inp_1";
        MLDictionaryFeatureProvider *provider = [[MLDictionaryFeatureProvider alloc]
            initWithDictionary:@{input_name: [MLFeatureValue featureValueWithMultiArray:array]} error:&e];
        if (!provider) { if (error_len) snprintf(error, error_len, "%s", e.localizedDescription.UTF8String); return 0; }
        id<MLFeatureProvider> result = [model predictionFromFeatures:provider error:&e];
        if (!result) { if (error_len) snprintf(error, error_len, "%s", e.localizedDescription.UTF8String); return 0; }
        NSString *name = result.featureNames.anyObject;
        MLMultiArray *out = [result featureValueForName:name].multiArrayValue;
        if (!out || out.count != 4 * 512 * 512) {
            snprintf(error, error_len, "unexpected Core ML output shape"); return 0;
        }
        // Fast path: a contiguous Float32 array is one memcpy. Reading a
        // million values through NSNumber boxing per tile was a visible share
        // of the per-image time. Any other layout takes the boxed path.
        const size_t bytes_wanted = 4 * 512 * 512 * sizeof(float);
        NSArray<NSNumber *> *strides = out.strides;
        BOOL contiguous = out.dataType == MLMultiArrayDataTypeFloat32 && strides.count == 4
            && strides[3].integerValue == 1 && strides[2].integerValue == 512
            && strides[1].integerValue == 512 * 512 && strides[0].integerValue == 4 * 512 * 512;
        if (contiguous) {
            __block BOOL copied = NO;
            [out getBytesWithHandler:^(const void *bytes, NSInteger size) {
                if ((size_t)size >= bytes_wanted) { memcpy(output, bytes, bytes_wanted); copied = YES; }
            }];
            if (copied) return 1;
        }
        // Strided or Float16 output (an fp16 ML Program may pad rows or keep
        // half precision): still one buffer pass, no per-value boxing.
        BOOL half = out.dataType == MLMultiArrayDataTypeFloat16;
        if ((half || out.dataType == MLMultiArrayDataTypeFloat32) && strides.count == 4
            && out.shape.count == 4) {
            const size_t element = half ? sizeof(__fp16) : sizeof(float);
            const size_t s1 = strides[1].unsignedIntegerValue, s2 = strides[2].unsignedIntegerValue,
                         s3 = strides[3].unsignedIntegerValue;
            const size_t last = 3 * s1 + 511 * s2 + 511 * s3;
            __block BOOL copied = NO;
            [out getBytesWithHandler:^(const void *bytes, NSInteger size) {
                if ((size_t)size < (last + 1) * element) return;
                for (size_t c = 0; c < 4; ++c)
                    for (size_t y = 0; y < 512; ++y) {
                        float *row = output + (c * 512 + y) * 512;
                        const size_t base = c * s1 + y * s2;
                        if (half) {
                            const __fp16 *values = (const __fp16 *)bytes;
                            for (size_t x = 0; x < 512; ++x) row[x] = values[base + x * s3];
                        } else {
                            const float *values = (const float *)bytes;
                            for (size_t x = 0; x < 512; ++x) row[x] = values[base + x * s3];
                        }
                    }
                copied = YES;
            }];
            if (copied) return 1;
        }
        for (NSInteger i = 0; i < out.count; ++i) output[i] = [out[i] floatValue];
        return 1;
    }
}

// Compile an .mlpackage on this Mac; writes the temporary .mlmodelc path.
int sp_ai_model_compile(const char *path, char *output, size_t output_len,
                        char *error, size_t error_len) {
    @autoreleasepool {
        NSError *e = nil;
        NSURL *compiled = [MLModel compileModelAtURL:[NSURL fileURLWithPath:
            [NSString stringWithUTF8String:path]] error:&e];
        if (!compiled) {
            if (error_len) snprintf(error, error_len, "%s", e.localizedDescription.UTF8String);
            return 0;
        }
        const char *result = compiled.fileSystemRepresentation;
        if (strlen(result) >= output_len) {
            if (error_len) snprintf(error, error_len, "compiled model path is too long");
            return 0;
        }
        snprintf(output, output_len, "%s", result);
        return 1;
    }
}

void sp_ai_model_close(void *handle) {
    if (handle) CFRelease(handle);
}

// Diagnostic used by acceptance tests; this reports the allowed policy,
// never claims which hardware ran an individual layer.
int sp_ai_model_configured_units(void *handle) {
    return (int)[(__bridge MLModel *)handle configuration].computeUnits;
}
