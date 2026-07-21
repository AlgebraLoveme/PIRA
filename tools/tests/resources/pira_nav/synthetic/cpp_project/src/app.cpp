#include "../include/widget.hpp"

int main() {
  demo::Widget widget("sample");
  return widget.name().empty();
}
