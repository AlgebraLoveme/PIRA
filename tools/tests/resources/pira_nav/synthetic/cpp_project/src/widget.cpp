#include "../include/widget.hpp"

#include <utility>

namespace demo {
Widget::Widget(std::string name) : name_(std::move(name)) {}

const std::string& Widget::name() const { return name_; }
}  // namespace demo
