"""CI workflows defined in code instead of YAML.

The builder produces exactly the file ``bsdkrun ci`` (and tangled's spindle)
consumes — :meth:`CIWorkflow.yaml` is that file, :meth:`CIWorkflow.save`
commits it to ``.tangled/workflows/``, and :meth:`CIWorkflow.run` executes it
in a microVM without a file ever touching the repository::

    from bsdkrun import ci

    ci.workflow("test") \\
        .on_push("main") \\
        .deps("python312", "uv") \\
        .env("CI_FROM", "sdk") \\
        .step("install", "uv sync") \\
        .step("test", "uv run pytest") \\
        .run()

Code is the source of truth and YAML the wire format, in that order — which is
why ``save()`` writes a generated-file header: a hand-edit there will be
overwritten by the next ``save()``.
"""

from __future__ import annotations

import json
import re
import subprocess
import tempfile
from dataclasses import dataclass, field
from pathlib import Path

from .binary import resolve_binary

__all__ = ["CIWorkflow", "workflow"]


def workflow(name: str) -> CIWorkflow:
    """Start a CI workflow definition."""
    return CIWorkflow(name)


@dataclass
class _Step:
    name: str
    command: str
    env: dict[str, str] = field(default_factory=dict)


class CIWorkflow:
    def __init__(self, name: str) -> None:
        self._name = name
        self._engine = "nixery"
        self._when: list[tuple[list[str], list[str]]] = []
        self._deps: dict[str, list[str]] = {}
        self._env: dict[str, str] = {}
        self._steps: list[_Step] = []
        self._clone_depth: int | None = None
        self._clone_skip = False

    # -- triggers ----------------------------------------------------------

    def engine(self, engine: str) -> CIWorkflow:
        """Override the engine (``nixery`` by default)."""
        self._engine = engine
        return self

    def on_push(self, *branches: str) -> CIWorkflow:
        """Add a push trigger for the given branches."""
        self._when.append((["push"], list(branches)))
        return self

    def on_pull_request(self, *branches: str) -> CIWorkflow:
        """Add a pull_request trigger targeting the given branches."""
        self._when.append((["pull_request"], list(branches)))
        return self

    def on(self, events: list[str], *branches: str) -> CIWorkflow:
        """Add a trigger with explicit events."""
        self._when.append((events, list(branches)))
        return self

    # -- contents ----------------------------------------------------------

    def deps(self, *packages: str) -> CIWorkflow:
        """Add nixpkgs dependencies — the toolchain the steps run against."""
        self._deps.setdefault("nixpkgs", []).extend(packages)
        return self

    def deps_from(self, registry: str, *packages: str) -> CIWorkflow:
        """Add dependencies from a custom registry (a flake reference)."""
        self._deps.setdefault(registry, []).extend(packages)
        return self

    def env(self, key: str, value: str) -> CIWorkflow:
        """Set a workflow-level environment variable."""
        self._env[key] = value
        return self

    def step(self, name: str, command: str, env: dict[str, str] | None = None) -> CIWorkflow:
        """Append a step; steps run serially in one VM, from the workspace."""
        self._steps.append(_Step(name, command, env or {}))
        return self

    def clone_depth(self, depth: int) -> CIWorkflow:
        """Set the clone depth (default 1)."""
        self._clone_depth = depth
        return self

    def skip_clone(self) -> CIWorkflow:
        """Skip the checkout entirely."""
        self._clone_skip = True
        return self

    # -- output ------------------------------------------------------------

    def file_name(self) -> str:
        """The workflow file name ``save()`` writes: ``<name>.yml``."""
        if re.search(r"\.ya?ml$", self._name):
            return self._name
        return f"{self._name}.yml"

    def yaml(self) -> str:
        """Render the workflow file.

        Scalars are emitted as JSON strings — valid YAML by construction —
        and commands as literal blocks when safe, so the SDK needs no YAML
        dependency.
        """
        q = json.dumps
        out: list[str] = []

        if self._when:
            out.append("when:")
            for events, branches in self._when:
                out.append(f"  - event: [{', '.join(q(e) for e in events)}]")
                if len(branches) == 1:
                    out.append(f"    branch: {q(branches[0])}")
                elif branches:
                    out.append(f"    branch: [{', '.join(q(b) for b in branches)}]")
            out.append("")

        out.append(f"engine: {self._engine}")

        if self._deps:
            out.extend(["", "dependencies:"])
            for reg in sorted(self._deps):
                out.append(f"  {q(reg)}:")
                out.extend(f"    - {q(p)}" for p in self._deps[reg])

        if self._env:
            out.extend(["", "environment:"])
            out.extend(f"  {k}: {q(self._env[k])}" for k in sorted(self._env))

        if self._clone_skip or self._clone_depth:
            out.extend(["", "clone:"])
            if self._clone_skip:
                out.append("  skip: true")
            if self._clone_depth:
                out.append(f"  depth: {self._clone_depth}")

        out.extend(["", "steps:"])
        for s in self._steps:
            out.append(f"  - name: {q(s.name)}")
            # Literal blocks read well in a committed file, but cannot carry
            # trailing spaces or carriage returns byte-for-byte; fall back to
            # a JSON string rather than silently altering the command.
            block_safe = (
                s.command != ""
                and "\r" not in s.command
                and all(ln == ln.rstrip(" ") for ln in s.command.split("\n"))
            )
            if block_safe:
                out.append("    command: |")
                out.extend(f"      {line}" for line in s.command.rstrip("\n").split("\n"))
            else:
                out.append(f"    command: {q(s.command)}")
            if s.env:
                out.append("    environment:")
                out.extend(f"      {k}: {q(s.env[k])}" for k in sorted(s.env))
        return "\n".join(out) + "\n"

    def save(self, repo: str | Path) -> Path:
        """Write into ``<repo>/.tangled/workflows/`` and return the path."""
        directory = Path(repo) / ".tangled" / "workflows"
        directory.mkdir(parents=True, exist_ok=True)
        path = directory / self.file_name()
        path.write_text(
            "# Generated by the bsdkrun SDK — edit the code that save()d it instead.\n"
            + self.yaml()
        )
        return path

    def run(self, directory: str | Path | None = None) -> None:
        """Execute the workflow in a microVM, streaming output.

        The YAML never touches the repository — it goes to a temp file and
        ``bsdkrun ci run -f``. Raises :class:`RuntimeError` when a step fails.
        """
        with tempfile.TemporaryDirectory(prefix="bsdkrun-ci-") as tmp:
            file = Path(tmp) / self.file_name()
            file.write_text(self.yaml())
            args = [resolve_binary(), "ci", "run", "-f", str(file)]
            if directory is not None:
                args += ["-w", str(directory)]
            # Inherit stdio, so step output streams exactly as a terminal run
            # of `bsdkrun ci` would.
            code = subprocess.call(args)
        if code != 0:
            raise RuntimeError(f"workflow {self._name} failed (exit {code})")
