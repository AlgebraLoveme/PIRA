#pragma once

struct ScaleConfig {
    float factor;
};

void launch_scale(float* values, int count, ScaleConfig config);
