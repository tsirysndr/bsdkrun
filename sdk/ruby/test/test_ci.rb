# frozen_string_literal: true

require "minitest/autorun"
require "bsdkrun"

# The YAML the builder emits is consumed by tangled's own workflow parser
# (inside `bsdkrun ci`), so these tests pin the emitted shape — a change here
# is a change to what spindle would receive.
class TestCI < Minitest::Test
  def test_full_workflow_shape
    y = Bsdkrun.workflow("test")
                .on_push("main")
                .on_pull_request("main", "develop")
                .deps("ruby", "bundler")
                .deps_from("github:nix-community/fenix/abc123", "stable.default")
                .env("CI_FROM", "sdk")
                .clone_depth(100)
                .step("install", "bundle install")
                .step("test", "bundle exec rake test", env: { "RACK_ENV" => "test" })
                .yaml

    assert_includes y, "  - event: [\"push\"]\n    branch: \"main\""
    assert_includes y, 'branch: ["main", "develop"]'
    assert_includes y, "engine: nixery"
    assert_includes y, "\"nixpkgs\":\n    - \"ruby\"\n    - \"bundler\""
    assert_includes y, '"github:nix-community/fenix/abc123":'
    assert_includes y, 'CI_FROM: "sdk"'
    assert_includes y, "depth: 100"
    assert_includes y, "- name: \"install\"\n    command: |\n      bundle install"
    assert_includes y, "environment:\n      RACK_ENV: \"test\""
  end

  def test_block_unsafe_command_falls_back_to_json
    # Trailing spaces do not survive a literal block scalar; the emitter must
    # switch representation rather than silently altering the command.
    y = Bsdkrun.workflow("edge").step("tricky", "echo 'a'  \necho b").yaml
    assert_includes y, "command: \"echo 'a'  \\necho b\""
  end

  def test_file_name
    assert_equal "build.yml", Bsdkrun.workflow("build").file_name
    assert_equal "build.yaml", Bsdkrun.workflow("build.yaml").file_name
  end
end
