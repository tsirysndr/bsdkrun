# frozen_string_literal: true

require_relative "lib/bsdkrun/version"

Gem::Specification.new do |spec|
  spec.name        = "bsdkrun"
  spec.version     = Bsdkrun::VERSION
  spec.authors     = ["Tsiry Sandratraina"]
  spec.email       = ["tsiry.sndr@gmail.com"]

  spec.summary     = "Ruby SDK for bsdkrun — a Firecracker-style microVM launcher for BSD, Linux, and unikernel guests."
  spec.description = <<~DESC
    A thin, dependency-free Ruby wrapper around the `bsdkrun` CLI. Boot and drive
    BSD/Linux/unikernel microVMs programmatically: create sandboxes, exec commands, manage
    lifecycle, and wire up global networks, volumes and images.
  DESC
  spec.homepage    = "https://github.com/tsirysndr/bsdkrun"
  spec.license     = "MIT"

  spec.required_ruby_version = ">= 3.2"

  spec.metadata["homepage_uri"]          = spec.homepage
  spec.metadata["source_code_uri"]       = "https://github.com/tsirysndr/bsdkrun/tree/main/sdk/ruby"
  spec.metadata["rubygems_mfa_required"] = "true"

  spec.files = Dir[
    "lib/**/*.rb",
    "README.md"
  ]
  spec.require_paths = ["lib"]

  # Runtime uses only the Ruby standard library (open3, json, pathname).

  spec.add_development_dependency "minitest", "~> 5.0"
  spec.add_development_dependency "rake", "~> 13.0"
end
