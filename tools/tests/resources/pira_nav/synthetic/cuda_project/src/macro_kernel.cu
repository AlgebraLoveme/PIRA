template <typename T>
PIRA_LAUNCH_BOUNDS(256)
__global__ void annotated_kernel(T* RESTRICT values) {
  auto normalize = [] PIRA_DEVICE_LAMBDA(T value) { return value; };
  values[0] = normalize(values[0]);
}
