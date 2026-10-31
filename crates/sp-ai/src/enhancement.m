// Named, shape-checked tensors for public enhancement models. This bridge is
// deliberately separate from the fixed camera-denoise tensor contract.
#import <CoreML/CoreML.h>

typedef struct {
    const char *name;
    const size_t *shape;
    size_t rank;
    float *data;
    size_t count;
} SPEnhanceTensor;

int sp_enhance_available(void) {
    if (@available(macOS 15.0, *)) return 1;
    return 0;
}

static int fail(char *error, size_t length, NSString *message) {
    if (length) snprintf(error, length, "%s", message.UTF8String);
    return 0;
}

int sp_enhance_predict(void *handle, const SPEnhanceTensor *inputs, size_t input_count,
                       SPEnhanceTensor *outputs, size_t output_count,
                       char *error, size_t error_len) {
    @autoreleasepool {
        @try {
            MLModel *model = (__bridge MLModel *)handle;
            NSMutableDictionary *features = [NSMutableDictionary new];
            NSError *e = nil;
            if (model.modelDescription.inputDescriptionsByName.count != input_count)
                return fail(error, error_len, @"Core ML input count differs from manifest");
            for (size_t i = 0; i < input_count; ++i) {
                const SPEnhanceTensor t = inputs[i];
                NSString *name = [NSString stringWithUTF8String:t.name];
                if (!model.modelDescription.inputDescriptionsByName[name])
                    return fail(error, error_len, @"Core ML input name differs from manifest");
                NSMutableArray *shape = [NSMutableArray new];
                NSMutableArray *strides = [NSMutableArray new];
                size_t stride = 1;
                for (size_t d = t.rank; d-- > 0;) {
                    [shape insertObject:@(t.shape[d]) atIndex:0];
                    [strides insertObject:@(stride) atIndex:0];
                    stride *= t.shape[d];
                }
                // Rust owns this contiguous buffer until synchronous prediction
                // returns. Core ML never frees it or retains a borrowed tensor.
                MLMultiArray *array = [[MLMultiArray alloc] initWithDataPointer:t.data
                    shape:shape dataType:MLMultiArrayDataTypeFloat32 strides:strides
                    deallocator:nil error:&e];
                if (!array) return fail(error, error_len, e.localizedDescription);
                features[name] = [MLFeatureValue featureValueWithMultiArray:array];
            }
            MLDictionaryFeatureProvider *provider = [[MLDictionaryFeatureProvider alloc]
                initWithDictionary:features error:&e];
            if (!provider) return fail(error, error_len, e.localizedDescription);
            id<MLFeatureProvider> result = [model predictionFromFeatures:provider error:&e];
            if (!result) return fail(error, error_len, e.localizedDescription);
            for (size_t i = 0; i < output_count; ++i) {
                SPEnhanceTensor t = outputs[i];
                MLMultiArray *array = [result featureValueForName:
                    [NSString stringWithUTF8String:t.name]].multiArrayValue;
                if (!array || array.shape.count != t.rank || (size_t)array.count != t.count)
                    return fail(error, error_len, @"Core ML output shape differs from manifest");
                BOOL contiguous = array.dataType == MLMultiArrayDataTypeFloat32;
                size_t stride = 1;
                for (size_t d = t.rank; d-- > 0;) {
                    if (array.shape[d].unsignedLongLongValue != t.shape[d])
                        return fail(error, error_len, @"Core ML output axis differs from manifest");
                    contiguous &= array.strides[d].unsignedLongLongValue == stride;
                    stride *= t.shape[d];
                }
                if (contiguous) {
                    memcpy(t.data, array.dataPointer, t.count * sizeof(float));
                } else {
                    // Core ML can return an FP16 or strided tensor. Logical
                    // subscripting honors its layout; never memcpy those bytes.
                    NSMutableArray<NSNumber *> *index = [NSMutableArray new];
                    for (size_t d = 0; d < t.rank; ++d) [index addObject:@0];
                    for (size_t n = 0; n < t.count; ++n) {
                        t.data[n] = [[array objectForKeyedSubscript:index] floatValue];
                        for (size_t d = t.rank; d-- > 0;) {
                            size_t next = index[d].unsignedLongLongValue + 1;
                            index[d] = @(next % t.shape[d]);
                            if (next < t.shape[d]) break;
                        }
                    }
                }
            }
            return 1;
        } @catch (NSException *exception) {
            return fail(error, error_len, exception.reason);
        }
    }
}
