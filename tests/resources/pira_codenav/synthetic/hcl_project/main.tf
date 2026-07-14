terraform {
  required_version = ">= 1.0"
}

module "child" {
  source = "./child/main.tf"
}

resource "example_widget" "main" {
  name = "demo"

  lifecycle {
    prevent_destroy = true
  }
}
