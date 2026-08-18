"""The YAML the builder emits is consumed by tangled's own workflow parser
(inside `bsdkrun ci`), so these tests pin the emitted shape — a change here is
a change to what spindle would receive."""

from bsdkrun import ci


def test_full_workflow_shape() -> None:
    y = (
        ci.workflow("test")
        .on_push("main")
        .on_pull_request("main", "develop")
        .deps("python312", "uv")
        .deps_from("github:nix-community/fenix/abc123", "stable.defaultToolchain")
        .env("CI_FROM", "sdk")
        .clone_depth(100)
        .step("install", "uv sync")
        .step("test", "uv run pytest", {"PYTHONDONTWRITEBYTECODE": "1"})
        .yaml()
    )
    assert '  - event: ["push"]\n    branch: "main"' in y
    assert 'branch: ["main", "develop"]' in y
    assert "engine: nixery" in y
    assert '"nixpkgs":\n    - "python312"\n    - "uv"' in y
    assert '"github:nix-community/fenix/abc123":' in y
    assert 'CI_FROM: "sdk"' in y
    assert "depth: 100" in y
    assert '- name: "install"\n    command: |\n      uv sync' in y
    assert 'environment:\n      PYTHONDONTWRITEBYTECODE: "1"' in y


def test_block_unsafe_command_falls_back_to_json() -> None:
    # Trailing spaces do not survive a literal block scalar; the emitter must
    # switch representation rather than silently altering the command.
    y = ci.workflow("edge").step("tricky", "echo 'a'  \necho b").yaml()
    assert "command: \"echo 'a'  \\necho b\"" in y


def test_file_name() -> None:
    assert ci.workflow("build").file_name() == "build.yml"
    assert ci.workflow("build.yaml").file_name() == "build.yaml"
