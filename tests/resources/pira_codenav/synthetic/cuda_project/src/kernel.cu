#include "../include/kernel.cuh"

namespace kernels {

__global__ void scale_kernel(float* values, int count, float factor) {
    int index = blockIdx.x * blockDim.x + threadIdx.x;
    if (index < count) {
        values[index] *= factor;
    }
}

void launch_scale(float* values, int count, ScaleConfig config) {
    scale_kernel<<<(count + 255) / 256, 256>>>(values, count, config.factor);
}

}  // namespace kernels
