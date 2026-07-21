normalize_name <- function(value) {
  trim <- function(x) {
    gsub("^\\s+|\\s+$", "", x)
  }

  tolower(trim(value))
}

(function(x) x * 2) -> double_value
