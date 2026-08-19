package main

// `--sh`: keep the CI machine alive when the run ends, and hand it to you.
//
// The loop this replaces is the reason the flag exists. A step fails in CI,
// and what you want is not the log — it is the machine: the same image, the
// same PATH, the same half-finished build tree, so you can run the failing
// command yourself and look around. Reproducing that by hand means guessing
// which container image, which environment variables and which working
// directory the runner used, and being wrong about one of them is what makes
// "works on my machine" a genre.
//
// So with `--sh` the machine is not torn down when the workflow finishes —
// pass or fail — and an interactive shell opens inside it, in the workspace,
// with the workflow's environment. The VM goes away when you exit that shell
// (`--keep` keeps it beyond that, for `bsdkrun shell` later).

import (
	"fmt"
	"os"
	"os/exec"
	"strings"

	"golang.org/x/term"
)

// envCaptureFile is where a step's final shell state is dumped when --sh is
// on, and where the interactive shell reads it back from. It lives under
// /tangled rather than $HOME because HOME is a tmpfs the run mounts.
const envCaptureFile = "/tangled/.bsdkrun-env"

// shellIntoVM opens an interactive shell in a finished workflow's machine and
// reports whether the machine must outlive this call. Failures here are
// reported and swallowed: the run's own verdict is what the caller returns,
// and a shell that could not open must not turn a passing workflow into a
// failing one.
func shellIntoVM(v *vm, plan *Plan, opts runOpts, runErr error) (keep bool) {
	if v == nil {
		return false
	}
	// Give the shell the environment the last step ended with. Without this
	// the shell is a fresh exec with only the image's environment: a run that
	// installed bun and exported its PATH would hand you a machine where
	// `bun` is not found, which is precisely the confusion --sh exists to
	// remove. Sourced from the shell's rc rather than passed as env, because
	// `bsdkrun shell` opens the shell itself.
	// A quoted heredoc carries the snippet verbatim: no quoting games, and
	// nothing in it is expanded by the shell that writes it.
	_, _ = v.exec([]string{"sh", "-c", fmt.Sprintf(`
for f in /root/.bashrc %[1]s/.bashrc /root/.profile %[1]s/.profile; do
  mkdir -p "$(dirname "$f")" 2>/dev/null || continue
  grep -q bsdkrun-env "$f" 2>/dev/null && continue
  cat >> "$f" <<'BSDKRUN_RC_EOF' 2>/dev/null || true
# bsdkrun-env: the environment the workflow's last step ended with
if [ -f %[2]s ]; then . %[2]s 2>/dev/null || true; fi
BSDKRUN_RC_EOF
done`, homeDir, envCaptureFile)}, nil, nil, nil)

	// The shell starts where the steps ran, not at /: `bsdkrun shell` reads
	// this file to pick its working directory.
	workdir := workspaceDir
	if plan != nil && plan.Workdir != "" {
		workdir = plan.Workdir
	}
	_, _ = v.exec([]string{"sh", "-c",
		fmt.Sprintf("printf '%%s\\n' %q > /etc/bsdkrun-cwd 2>/dev/null || true", workdir)},
		nil, nil, nil)

	// Without a terminal there is nobody to hand the shell to — a CI job, a
	// pipe, an editor task. Say what to do instead of opening something that
	// would immediately read EOF and exit.
	if !term.IsTerminal(int(os.Stdin.Fd())) {
		logf(opts, "\n--sh: no terminal on stdin, so the machine is left running instead.\n")
		logf(opts, "      bsdkrun shell %s      # attach\n", v.ID)
		logf(opts, "      bsdkrun rm -f %s      # when you are done\n", v.ID)
		// Kept, because that is what the line above just promised — and
		// because --sh's whole contract is that the machine survives the run.
		return true
	}

	fmt.Fprintln(os.Stderr)
	if runErr != nil {
		fmt.Fprintf(os.Stderr, "--sh: %s failed — dropping you into the machine it failed in.\n", plan.Name)
	} else {
		fmt.Fprintf(os.Stderr, "--sh: %s finished — dropping you into its machine.\n", plan.Name)
	}
	fmt.Fprintf(os.Stderr, "      machine %s, image %s, workspace %s\n", v.ID, plan.Image, workdir)
	if cmd := lastUserStep(plan); cmd != "" {
		fmt.Fprintf(os.Stderr, "      last step ran: %s\n", cmd)
	}
	fmt.Fprintf(os.Stderr, "      exit the shell to destroy the machine.\n\n")

	// `bsdkrun shell` owns the PTY and the raw-mode handling; inheriting the
	// standard streams is all this side has to do.
	c := exec.Command(bsdkrunBin(), "shell", v.ID)
	c.Stdin, c.Stdout, c.Stderr = os.Stdin, os.Stdout, os.Stderr
	if err := c.Run(); err != nil {
		// An exit status from the shell itself is normal (you typed `exit 1`),
		// and not something to report as a failure of the run.
		if _, ok := err.(*exec.ExitError); !ok {
			fmt.Fprintf(os.Stderr, "--sh: could not open a shell in %s: %v\n", v.ID, err)
			// The shell never opened, so nothing has been inspected yet:
			// keep the machine rather than destroying it unseen.
			return true
		}
	}
	return false
}

// lastUserStep is the command a reader most likely wants to re-run by hand:
// the final step the workflow declared. Truncated, because a step body can be
// a whole script.
func lastUserStep(plan *Plan) string {
	if plan == nil {
		return ""
	}
	for i := len(plan.Steps) - 1; i >= 0; i-- {
		if plan.Steps[i].System {
			continue
		}
		cmd := strings.TrimSpace(plan.Steps[i].Command)
		if cmd == "" {
			continue
		}
		if idx := strings.IndexByte(cmd, '\n'); idx >= 0 {
			cmd = cmd[:idx] + " …"
		}
		if len(cmd) > 72 {
			cmd = cmd[:72] + "…"
		}
		return cmd
	}
	return ""
}
