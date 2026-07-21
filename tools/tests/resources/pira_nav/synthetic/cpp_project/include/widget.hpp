#pragma once

#include <string>

namespace demo {
class Widget {
 public:
  explicit Widget(std::string name);
  const std::string& name() const;

 private:
  std::string name_;
};
}  // namespace demo
