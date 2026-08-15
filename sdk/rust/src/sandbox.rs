//! The [`Sandbox`] handle — a running (or stopped) bsdkrun microVM — and the
//! fluent builders that boot one.
//!
//! ```no_run
//! use bsdkrun_sdk::Sandbox;
//!
//! let boxx = Sandbox::linux("alpine")
//!     .cpus(2)
//!     .mem(1024)
//!     .port("8080:80")
//!     .command(["sleep", "300"])
//!     .create()?;
//! println!("{}", boxx.exec(["uname", "-a"])?.text());
//! boxx.stop()?;
//! # Ok::<(), bsdkrun_sdk::Error>(())
//! ```

use serde_json::Value;

use crate::args::{build_create_args, strvec, CreateOpts, Kind};
use crate::error::{Error, Result};
use crate::process::{checked, run_full, run_full_stream, spawn};
use crate::types::{ExecResult, SandboxInfo};

/// A machine id as the CLI prints it: lowercase hex, at least six digits, on a
/// line of its own.
fn looks_like_machine_id(line: &str) -> bool {
    line.len() >= 6
        && line
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

/// The host SSH port from the boot banner's `ssh -p <port>` hint, if any.
fn parse_ssh_port(stderr: &str) -> Option<u16> {
    let idx = stderr.find("ssh -p ")?;
    let digits: String = stderr[idx + "ssh -p ".len()..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

/// A handle to a running (or stopped) bsdkrun microVM.
///
/// Create one with the per-kind builders ([`Sandbox::linux`],
/// [`Sandbox::freebsd`], ...), reconnect with [`Sandbox::get`], or enumerate
/// with [`Sandbox::list`].
#[derive(Debug, Clone)]
pub struct Sandbox {
    id: String,
    ssh_port: Option<u16>,
}

// -- shared setter macros -----------------------------------------------------
//
// Each create builder wraps the same `CreateOpts`; these macros stamp the
// option groups (net/name/vm, disks) into each builder's impl so the groups
// cannot drift between kinds — the same reason args.py factors them into
// _net_args/_vm_args/_disk_args.

macro_rules! common_setters {
    () => {
        /// Name the machine (`--name`).
        pub fn name(mut self, name: impl Into<String>) -> Self {
            self.opts.name = Some(name.into());
            self
        }

        /// vCPU count (`--cpus`).
        pub fn cpus(mut self, cpus: u32) -> Self {
            self.opts.cpus = Some(cpus);
            self
        }

        /// Guest RAM in MiB (`--mem`).
        pub fn mem(mut self, mib: u32) -> Self {
            self.opts.mem = Some(mib);
            self
        }

        /// Add a host->guest TCP port forward, `"HOST:GUEST"`.
        pub fn port(mut self, forward: impl Into<String>) -> Self {
            self.opts.net.touched = true;
            self.opts.net.ports.push(forward.into());
            self
        }

        /// Add a port forward from numbers instead of a string.
        pub fn forward(self, host: u16, guest: u16) -> Self {
            self.port(format!("{host}:{guest}"))
        }

        /// Pin the guest MAC address (`--mac`).
        pub fn mac(mut self, mac: impl Into<String>) -> Self {
            self.opts.net.touched = true;
            self.opts.net.mac = Some(mac.into());
            self
        }

        /// Join a global network (`--network`).
        pub fn network(mut self, network: impl Into<String>) -> Self {
            self.opts.net.touched = true;
            self.opts.net.network = Some(network.into());
            self
        }

        /// Disable guest networking entirely (`--no-net`).
        pub fn no_net(mut self) -> Self {
            self.opts.net.touched = true;
            self.opts.net.no_net = true;
            self
        }

        /// bsdkrun's global `--log-level` for the create call (default 1, so
        /// boot diagnostics land in the error when a boot fails).
        pub fn log_level(mut self, level: u32) -> Self {
            self.opts.log_level = Some(level);
            self
        }

        /// The exact argv `create()` will run (minus the binary and global
        /// flags) — for inspection and tests.
        pub fn to_args(&self) -> Vec<String> {
            build_create_args(self.kind, &self.opts)
        }

        /// Boot the machine (detached) and return a handle to it.
        pub fn create(self) -> Result<Sandbox> {
            Sandbox::create_with(self.kind, self.opts)
        }
    };
}

macro_rules! disk_setters {
    () => {
        /// Keep the root disk across `rm` (`--persist`).
        pub fn persist(mut self) -> Self {
            self.opts.persist = true;
            self
        }

        /// Use a persistent CoW volume as the root disk (`-v`).
        pub fn volume(mut self, name: impl Into<String>) -> Self {
            self.opts.volume = Some(name.into());
            self
        }

        /// Attach an extra raw disk, `"PATH"` or `"PATH:ro"` (`--attach-disk`).
        pub fn attach_disk(mut self, disk: impl Into<String>) -> Self {
            self.opts.attach_disk.push(disk.into());
            self
        }
    };
}

macro_rules! define_builder {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone)]
        pub struct $name {
            kind: Kind,
            opts: CreateOpts,
        }
    };
}

define_builder!(
    /// Boots an OCI image as a Linux microVM (`bsdkrun linux`).
    LinuxBuilder
);
define_builder!(
    /// Boots FreeBSD (`bsdkrun freebsd`).
    FreebsdBuilder
);
define_builder!(
    /// Boots NetBSD (`bsdkrun netbsd`).
    NetbsdBuilder
);
define_builder!(
    /// Boots a raw disk through its UEFI loader (`bsdkrun firmware`).
    FirmwareBuilder
);
define_builder!(
    /// Boots a kernel directly, no bootloader (`bsdkrun kernel`).
    KernelBuilder
);
define_builder!(
    /// Boots a Nanos unikernel image (`bsdkrun nanos`).
    NanosBuilder
);
define_builder!(
    /// Boots an OSv unikernel image (`bsdkrun osv`).
    OsvBuilder
);
define_builder!(
    /// Boots a Unikraft unikernel (`bsdkrun unikraft`).
    UnikraftBuilder
);
define_builder!(
    /// Boots a Solo5 (MirageOS) unikernel under the `solo5-hvt` tender
    /// (`bsdkrun solo5`).
    Solo5Builder
);

impl LinuxBuilder {
    common_setters!();

    /// Custom kernel image (`--kernel`).
    pub fn kernel(mut self, kernel: impl Into<String>) -> Self {
        self.opts.kernel = Some(kernel.into());
        self
    }

    /// Kernel version to fetch (`--kernel-version`).
    pub fn kernel_version(mut self, version: impl Into<String>) -> Self {
        self.opts.kernel_version = Some(version.into());
        self
    }

    /// Boot through an initramfs (`--initramfs`, a bare flag for Linux).
    pub fn initramfs(mut self) -> Self {
        self.opts.initramfs_flag = true;
        self
    }

    /// Use a persistent CoW volume as the rootfs (`-v`).
    pub fn volume(mut self, name: impl Into<String>) -> Self {
        self.opts.volume = Some(name.into());
        self
    }

    /// Share a host directory into the guest, `"HOST:GUEST"` or
    /// `"HOST:GUEST:ro"` (`--mount`, repeatable).
    pub fn mount(mut self, mount: impl Into<String>) -> Self {
        self.opts.mounts.push(mount.into());
        self
    }

    /// Attach an extra raw disk as virtio-blk, `"PATH"` or `"PATH:ro"`
    /// (`--attach-disk`, repeatable). With the default virtio-fs rootfs the
    /// first attachment is the guest's `/dev/vda` — format and mount it in the
    /// guest for native-speed I/O.
    pub fn attach_disk(mut self, disk: impl Into<String>) -> Self {
        self.opts.attach_disk.push(disk.into());
        self
    }

    /// Override the image entrypoint (`--entrypoint`).
    pub fn entrypoint(mut self, entrypoint: impl Into<String>) -> Self {
        self.opts.entrypoint = Some(entrypoint.into());
        self
    }

    /// Console device (`--console`).
    pub fn console(mut self, console: impl Into<String>) -> Self {
        self.opts.console = Some(console.into());
        self
    }

    /// The command to run in the guest (argv after `--`).
    pub fn command<I, S>(mut self, command: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.opts.command = strvec(command);
        self
    }
}

impl FreebsdBuilder {
    common_setters!();
    disk_setters!();

    /// FreeBSD release to boot (`--version`).
    pub fn version(mut self, version: impl Into<String>) -> Self {
        self.opts.version = Some(version.into());
        self
    }

    /// Custom EFI firmware (`--firmware`).
    pub fn firmware(mut self, firmware: impl Into<String>) -> Self {
        self.opts.firmware = Some(firmware.into());
        self
    }

    /// Re-fetch the image even if cached (`--force`).
    pub fn force(mut self) -> Self {
        self.opts.force = true;
        self
    }
}

impl NetbsdBuilder {
    common_setters!();
    disk_setters!();

    /// NetBSD release to boot (`--version`).
    pub fn version(mut self, version: impl Into<String>) -> Self {
        self.opts.version = Some(version.into());
        self
    }

    /// Re-fetch the image even if cached (`--force`).
    pub fn force(mut self) -> Self {
        self.opts.force = true;
        self
    }
}

impl FirmwareBuilder {
    common_setters!();
    disk_setters!();
}

impl KernelBuilder {
    common_setters!();
    disk_setters!();

    /// Kernel image format (`--format`, e.g. `"elf"`).
    pub fn format(mut self, format: impl Into<String>) -> Self {
        self.opts.format = Some(format.into());
        self
    }

    /// Initramfs image path (`--initramfs` takes a value for direct-kernel
    /// boots, unlike the Linux builder's bare flag).
    pub fn initramfs(mut self, path: impl Into<String>) -> Self {
        self.opts.initramfs_path = Some(path.into());
        self
    }

    /// Kernel command line (`--cmdline`).
    pub fn cmdline(mut self, cmdline: impl Into<String>) -> Self {
        self.opts.cmdline = Some(cmdline.into());
        self
    }

    /// Root disk (`--disk`).
    pub fn disk(mut self, disk: impl Into<String>) -> Self {
        self.opts.disk = Some(disk.into());
        self
    }
}

impl NanosBuilder {
    common_setters!();

    /// Nanos kernel override (`--kernel`, Linux hosts).
    pub fn kernel(mut self, kernel: impl Into<String>) -> Self {
        self.opts.kernel = Some(kernel.into());
        self
    }

    /// Kernel command line (`--cmdline`).
    pub fn cmdline(mut self, cmdline: impl Into<String>) -> Self {
        self.opts.cmdline = Some(cmdline.into());
        self
    }

    /// Keep the root disk across `rm` (`--persist`).
    pub fn persist(mut self) -> Self {
        self.opts.persist = true;
        self
    }
}

impl OsvBuilder {
    common_setters!();

    /// The application to run and its arguments (`--cmdline`, e.g. `"/hello.so"`).
    pub fn cmdline(mut self, cmdline: impl Into<String>) -> Self {
        self.opts.cmdline = Some(cmdline.into());
        self
    }

    /// Root disk, required on x86_64 (`--disk`).
    pub fn disk(mut self, disk: impl Into<String>) -> Self {
        self.opts.disk = Some(disk.into());
        self
    }

    /// GIC version, aarch64 only (`--gic`, `"v2"` or `"v3"`).
    pub fn gic(mut self, gic: impl Into<String>) -> Self {
        self.opts.gic = Some(gic.into());
        self
    }

    /// Keep the root disk across `rm` (`--persist`).
    pub fn persist(mut self) -> Self {
        self.opts.persist = true;
        self
    }
}

impl UnikraftBuilder {
    common_setters!();

    /// Kernel command line; Unikraft hands it to the application as argv
    /// (`--cmdline`).
    pub fn cmdline(mut self, cmdline: impl Into<String>) -> Self {
        self.opts.cmdline = Some(cmdline.into());
        self
    }

    /// Initramfs image path (`--initramfs`).
    pub fn initramfs(mut self, path: impl Into<String>) -> Self {
        self.opts.initramfs_path = Some(path.into());
        self
    }

    /// A virtio-fs share, `"HOST:GUEST"` with an absolute guest path
    /// (`--mount`, repeatable). Needs a unikernel built for it.
    pub fn mount(mut self, mount: impl Into<String>) -> Self {
        self.opts.mounts.push(mount.into());
        self
    }
}

impl Solo5Builder {
    common_setters!();

    /// Backing file for a declared block device, `"NAME=FILE"` (`--block`,
    /// repeatable). The `NAME=` may be omitted when the unikernel declares
    /// exactly one.
    pub fn block(mut self, block: impl Into<String>) -> Self {
        self.opts.block.push(block.into());
        self
    }

    /// Arguments passed to the unikernel itself, after a literal `--` (e.g.
    /// MirageOS's `--ipv4=10.0.0.2/24`).
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.opts.trailing_args = strvec(args);
        self
    }
}

// -- Sandbox ------------------------------------------------------------------

impl Sandbox {
    /// Read and write files in the guest.
    pub fn fs(&self) -> crate::FileSystem {
        crate::filesystem::FileSystem::new(&self.id)
    }

    /// Boot an OCI image as a Linux microVM.
    pub fn linux(image: impl Into<String>) -> LinuxBuilder {
        LinuxBuilder {
            kind: Kind::Linux,
            opts: CreateOpts {
                image: Some(image.into()),
                ..Default::default()
            },
        }
    }

    /// Boot FreeBSD (EFI on macOS, PVH on Linux/amd64).
    pub fn freebsd() -> FreebsdBuilder {
        FreebsdBuilder {
            kind: Kind::Freebsd,
            opts: CreateOpts::default(),
        }
    }

    /// Boot NetBSD (direct-kernel boot everywhere).
    pub fn netbsd() -> NetbsdBuilder {
        NetbsdBuilder {
            kind: Kind::Netbsd,
            opts: CreateOpts::default(),
        }
    }

    /// Boot a raw disk through its UEFI loader.
    pub fn firmware(firmware: impl Into<String>, disk: impl Into<String>) -> FirmwareBuilder {
        FirmwareBuilder {
            kind: Kind::Firmware,
            opts: CreateOpts {
                firmware: Some(firmware.into()),
                disk: Some(disk.into()),
                ..Default::default()
            },
        }
    }

    /// Boot a kernel directly, no bootloader.
    pub fn kernel(kernel: impl Into<String>) -> KernelBuilder {
        KernelBuilder {
            kind: Kind::Kernel,
            opts: CreateOpts {
                kernel: Some(kernel.into()),
                ..Default::default()
            },
        }
    }

    /// Boot a Nanos image (a path, or a bare name in `~/.ops/images`).
    pub fn nanos(image: impl Into<String>) -> NanosBuilder {
        NanosBuilder {
            kind: Kind::Nanos,
            opts: CreateOpts {
                image: Some(image.into()),
                ..Default::default()
            },
        }
    }

    /// Boot an OSv image (an aarch64 `loader.img`, or on x86_64 the loader
    /// ELF plus a [`OsvBuilder::disk`]).
    pub fn osv(image: impl Into<String>) -> OsvBuilder {
        OsvBuilder {
            kind: Kind::Osv,
            opts: CreateOpts {
                image: Some(image.into()),
                ..Default::default()
            },
        }
    }

    /// Boot a Unikraft unikernel (a kraft project dir or a built image).
    pub fn unikraft(path: impl Into<String>) -> UnikraftBuilder {
        UnikraftBuilder {
            kind: Kind::Unikraft,
            opts: CreateOpts {
                path: Some(path.into()),
                ..Default::default()
            },
        }
    }

    /// Boot a Solo5 (MirageOS) unikernel (a `.hvt` binary or a project dir
    /// whose `dist/` holds one).
    pub fn solo5(path: impl Into<String>) -> Solo5Builder {
        Solo5Builder {
            kind: Kind::Solo5,
            opts: CreateOpts {
                path: Some(path.into()),
                ..Default::default()
            },
        }
    }

    fn create_with(kind: Kind, opts: CreateOpts) -> Result<Sandbox> {
        // Default log level 1 (boot diagnostics), unlike every other call:
        // when a boot fails, the diagnostics are the error message.
        let log_level = opts.log_level.unwrap_or(1);
        let args = build_create_args(kind, &opts);
        let res = run_full(&args, &[], None, log_level)?;
        if res.exit_code != 0 {
            return Err(Error::CommandFailed {
                exit_code: res.exit_code,
                stdout: res.stdout,
                stderr: res.stderr,
                command: "bsdkrun create".to_string(),
            });
        }

        // Detached runs print just the machine id on stdout — take the last
        // line that looks like one.
        let machine_id = res
            .stdout
            .lines()
            .map(str::trim)
            .rfind(|line| looks_like_machine_id(line))
            .map(str::to_string);
        let Some(id) = machine_id else {
            return Err(Error::CommandFailed {
                exit_code: res.exit_code,
                stdout: res.stdout,
                stderr: res.stderr,
                command: "bsdkrun create (no machine id in output)".to_string(),
            });
        };
        let ssh_port = parse_ssh_port(&res.stderr);
        Ok(Sandbox { id, ssh_port })
    }

    /// Wrap an already-known machine id without checking it exists.
    pub fn from_id(id: impl Into<String>) -> Sandbox {
        Sandbox {
            id: id.into(),
            ssh_port: None,
        }
    }

    /// Reconnect to an existing machine by id (a unique prefix is enough).
    pub fn get(id: &str) -> Result<Sandbox> {
        for info in Self::list(true)? {
            if info.id == id || info.id.starts_with(id) || info.name == Some(id.to_string()) {
                return Ok(Sandbox {
                    id: info.id,
                    ssh_port: None,
                });
            }
        }
        Err(Error::SandboxNotFound { id: id.to_string() })
    }

    /// List machines. `all` includes exited ones (otherwise: running only).
    pub fn list(all: bool) -> Result<Vec<SandboxInfo>> {
        let mut args = vec!["ps".to_string(), "--json".to_string()];
        if all {
            args.push("--all".to_string());
        }
        let res = checked(&args, "bsdkrun ps")?;
        let raw = if res.stdout.trim().is_empty() {
            "[]".to_string()
        } else {
            res.stdout
        };
        let rows: Value = serde_json::from_str(&raw)?;
        Ok(rows
            .as_array()
            .map(|rows| rows.iter().map(SandboxInfo::from_row).collect())
            .unwrap_or_default())
    }

    /// The machine's Docker-style short id.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Host port forwarded to the guest's SSH, if the boot banner reported one.
    pub fn ssh_port(&self) -> Option<u16> {
        self.ssh_port
    }

    // -- commands ----------------------------------------------------------

    /// Start building a guest command: program first, everything else chained.
    ///
    /// ```no_run
    /// # let sandbox = bsdkrun_sdk::Sandbox::from_id("abc123");
    /// let out = sandbox
    ///     .command("node")
    ///     .args(["-e", "console.log(1)"])
    ///     .env("X", "hi")
    ///     .cwd("/app")
    ///     .run()?;
    /// # Ok::<(), bsdkrun_sdk::Error>(())
    /// ```
    pub fn command(&self, program: impl Into<String>) -> CommandBuilder {
        CommandBuilder {
            sandbox_id: self.id.clone(),
            argv: vec![program.into()],
            env: Vec::new(),
            cwd: None,
            stdin: None,
            tty: false,
            log_level: 0,
            stdout: None,
            stderr: None,
        }
    }

    /// Run an argv in the guest through its exec agent — the shorthand for
    /// [`Sandbox::command`] with no extra options.
    ///
    /// A non-zero exit is reported in the [`ExecResult`], not raised; chain
    /// [`ExecResult::ok_or_err`] to turn it into an error.
    pub fn exec<I, S>(&self, argv: I) -> Result<ExecResult>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut argv = strvec(argv).into_iter();
        let Some(program) = argv.next() else {
            return Err(Error::InvalidInput("exec needs a non-empty argv".into()));
        };
        let mut builder = self.command(program);
        builder.argv.extend(argv);
        builder.run()
    }

    /// Read the machine's console log.
    pub fn logs(&self) -> Result<String> {
        Ok(run_full(&strvec(["logs", self.id.as_str()]), &[], None, 0)?.stdout)
    }

    /// Read bsdkrun's boot log for the machine.
    pub fn boot_logs(&self) -> Result<String> {
        Ok(run_full(&strvec(["logs", "--boot", self.id.as_str()]), &[], None, 0)?.stdout)
    }

    /// Attach an interactive shell to the machine (inherits the terminal).
    ///
    /// Blocks until the shell exits and returns its exit code.
    pub fn shell(&self) -> Result<i32> {
        spawn(["shell", self.id.as_str()])
    }

    // -- inspection --------------------------------------------------------

    /// Fetch this machine's current status row, or `None` if it's gone.
    pub fn status(&self) -> Result<Option<SandboxInfo>> {
        Ok(Self::list(true)?
            .into_iter()
            .find(|info| info.id == self.id))
    }

    /// Whether the machine is currently running.
    pub fn is_running(&self) -> Result<bool> {
        Ok(self.status()?.map(|info| info.running).unwrap_or(false))
    }

    // -- lifecycle ---------------------------------------------------------

    /// Stop the machine (BSD: clean power-off; Linux: SIGTERM).
    pub fn stop(&self) -> Result<()> {
        checked(&strvec(["stop", self.id.as_str()]), "bsdkrun stop")?;
        Ok(())
    }

    /// Restart a stopped machine in place — same id, disk/rootfs, network.
    pub fn start(&self) -> Result<()> {
        checked(&strvec(["start", self.id.as_str()]), "bsdkrun start")?;
        Ok(())
    }

    /// Remove the machine and its state. `force` stops it first if running.
    pub fn remove(&self, force: bool) -> Result<()> {
        let mut args = vec!["rm".to_string()];
        if force {
            args.push("--force".to_string());
        }
        args.push(self.id.clone());
        checked(&args, "bsdkrun rm")?;
        Ok(())
    }

    /// Change the recorded vCPU / RAM; applies on the next [`Sandbox::start`].
    ///
    /// ```no_run
    /// # let sandbox = bsdkrun_sdk::Sandbox::from_id("abc123");
    /// sandbox.update().cpus(4).mem(2048).apply()?;
    /// # Ok::<(), bsdkrun_sdk::Error>(())
    /// ```
    pub fn update(&self) -> UpdateBuilder {
        UpdateBuilder {
            id: self.id.clone(),
            cpus: None,
            mem: None,
        }
    }

    /// Join or switch this machine to a global network (next start).
    pub fn connect_network(&self, network: &str) -> Result<()> {
        checked(
            &strvec(["network", "connect", self.id.as_str(), network]),
            "bsdkrun network connect",
        )?;
        Ok(())
    }

    /// Detach this machine from its network. Applies on the next start.
    pub fn disconnect_network(&self) -> Result<()> {
        checked(
            &strvec(["network", "disconnect", self.id.as_str()]),
            "bsdkrun network disconnect",
        )?;
        Ok(())
    }

    // -- in-guest agent helpers -------------------------------------------

    /// Install SSH keys in the guest (`ssh setup`, via the agent).
    ///
    /// With no key, the CLI installs your local `~/.ssh/*.pub` keys.
    ///
    /// ```no_run
    /// # let sandbox = bsdkrun_sdk::Sandbox::from_id("abc123");
    /// sandbox.ssh_setup().user("tsiry").key("~/.ssh/work.pub").run()?;
    /// # Ok::<(), bsdkrun_sdk::Error>(())
    /// ```
    pub fn ssh_setup(&self) -> SshSetupBuilder {
        SshSetupBuilder {
            id: self.id.clone(),
            user: None,
            keys: Vec::new(),
        }
    }

    /// Put the guest on your tailnet (`tailscale setup`, via the agent).
    ///
    /// ```no_run
    /// # let sandbox = bsdkrun_sdk::Sandbox::from_id("abc123");
    /// sandbox.tailscale_up().authkey("tskey-auth-...").hostname("web").run()?;
    /// # Ok::<(), bsdkrun_sdk::Error>(())
    /// ```
    pub fn tailscale_up(&self) -> TailscaleUpBuilder {
        TailscaleUpBuilder {
            id: self.id.clone(),
            authkey: None,
            hostname: None,
            extra: Vec::new(),
        }
    }
}

/// A guest command being assembled — see [`Sandbox::command`].
pub struct CommandBuilder {
    sandbox_id: String,
    argv: Vec<String>,
    env: Vec<(String, String)>,
    cwd: Option<String>,
    stdin: Option<Vec<u8>>,
    tty: bool,
    log_level: u32,
    stdout: Option<Box<dyn std::io::Write + Send>>,
    stderr: Option<Box<dyn std::io::Write + Send>>,
}

impl CommandBuilder {
    /// Append one argument.
    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.argv.push(arg.into());
        self
    }

    /// Append arguments.
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.argv.extend(strvec(args));
        self
    }

    /// Set a per-command environment variable (`-e K=V`).
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    /// Run in a working directory (emulated: `cd`, then exec the real argv).
    pub fn cwd(mut self, cwd: impl Into<String>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    /// Pipe bytes (or a string) to the command's stdin.
    pub fn stdin(mut self, data: impl AsRef<[u8]>) -> Self {
        self.stdin = Some(data.as_ref().to_vec());
        self
    }

    /// Allocate a PTY (`-t`).
    pub fn tty(mut self, tty: bool) -> Self {
        self.tty = tty;
        self
    }

    /// bsdkrun's global `--log-level` for this call (default 0 — quiet).
    pub fn log_level(mut self, level: u32) -> Self {
        self.log_level = level;
        self
    }

    /// Stream stdout to `writer` while retaining it in the returned result.
    pub fn stdout(mut self, writer: impl std::io::Write + Send + 'static) -> Self {
        self.stdout = Some(Box::new(writer));
        self
    }

    /// Stream stderr to `writer` while retaining it in the returned result.
    pub fn stderr(mut self, writer: impl std::io::Write + Send + 'static) -> Self {
        self.stderr = Some(Box::new(writer));
        self
    }

    /// Run the command to completion and capture the result.
    ///
    /// A non-zero exit is data, not an error — chain
    /// [`ExecResult::ok_or_err`] when it should be one.
    pub fn run(self) -> Result<ExecResult> {
        let mut argv = self.argv;
        if let Some(cwd) = &self.cwd {
            // Emulate a working directory: cd, drop it, then exec the real argv.
            let mut wrapped = strvec([
                "/bin/sh",
                "-c",
                "cd \"$1\" && shift && exec \"$@\"",
                "sh",
                cwd.as_str(),
            ]);
            wrapped.append(&mut argv);
            argv = wrapped;
        }

        let mut cli = vec!["exec".to_string()];
        if self.tty {
            cli.push("-t".to_string());
        }
        for (key, value) in &self.env {
            cli.push("-e".to_string());
            cli.push(format!("{key}={value}"));
        }
        cli.push(self.sandbox_id.clone());
        cli.extend(argv.iter().cloned());

        let res = run_full_stream(
            &cli,
            &[],
            self.stdin.as_deref(),
            self.log_level,
            self.stdout,
            self.stderr,
        )?;
        Ok(ExecResult {
            stdout: res.stdout,
            stderr: res.stderr,
            exit_code: res.exit_code,
            command: format!("exec {}", argv.join(" ")),
        })
    }
}

/// Pending vCPU / RAM changes — see [`Sandbox::update`].
#[derive(Debug, Clone)]
pub struct UpdateBuilder {
    id: String,
    cpus: Option<u32>,
    mem: Option<u32>,
}

impl UpdateBuilder {
    pub fn cpus(mut self, cpus: u32) -> Self {
        self.cpus = Some(cpus);
        self
    }

    pub fn mem(mut self, mib: u32) -> Self {
        self.mem = Some(mib);
        self
    }

    /// Record the change; it applies on the machine's next start.
    pub fn apply(self) -> Result<()> {
        let mut args = strvec(["update", self.id.as_str()]);
        if let Some(cpus) = self.cpus {
            args.push("--cpus".to_string());
            args.push(cpus.to_string());
        }
        if let Some(mem) = self.mem {
            args.push("--mem".to_string());
            args.push(mem.to_string());
        }
        checked(&args, "bsdkrun update")?;
        Ok(())
    }
}

/// An `ssh setup` invocation being assembled — see [`Sandbox::ssh_setup`].
#[derive(Debug, Clone)]
pub struct SshSetupBuilder {
    id: String,
    user: Option<String>,
    keys: Vec<String>,
}

impl SshSetupBuilder {
    /// The guest user to install keys for.
    pub fn user(mut self, user: impl Into<String>) -> Self {
        self.user = Some(user.into());
        self
    }

    /// A literal `ssh-...` public key, or a local `.pub` path (repeatable).
    pub fn key(mut self, key: impl Into<String>) -> Self {
        self.keys.push(key.into());
        self
    }

    /// Run the setup; a non-zero exit is an error here (unlike `exec`),
    /// because a failed key install has nothing useful to report but failure.
    pub fn run(self) -> Result<ExecResult> {
        let mut action = vec!["setup".to_string()];
        if let Some(user) = &self.user {
            action.push("--user".to_string());
            action.push(user.clone());
        }
        for key in &self.keys {
            action.push("--key".to_string());
            action.push(key.clone());
        }
        run_agent("ssh", &self.id, &action, &[])
    }
}

/// A `tailscale setup` invocation being assembled — see [`Sandbox::tailscale_up`].
#[derive(Debug, Clone)]
pub struct TailscaleUpBuilder {
    id: String,
    authkey: Option<String>,
    hostname: Option<String>,
    extra: Vec<String>,
}

impl TailscaleUpBuilder {
    /// A tailnet auth key — forwarded as the `TS_AUTHKEY` env var, kept off
    /// the argument list so it never lands in a process listing.
    pub fn authkey(mut self, authkey: impl Into<String>) -> Self {
        self.authkey = Some(authkey.into());
        self
    }

    /// The machine name on the tailnet.
    pub fn hostname(mut self, hostname: impl Into<String>) -> Self {
        self.hostname = Some(hostname.into());
        self
    }

    /// Append a raw extra argument to the setup call.
    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.extra.push(arg.into());
        self
    }

    /// Run the setup; a non-zero exit is an error.
    pub fn run(self) -> Result<ExecResult> {
        let mut action = vec!["setup".to_string()];
        if let Some(hostname) = &self.hostname {
            action.push("--hostname".to_string());
            action.push(hostname.clone());
        }
        action.extend(self.extra.iter().cloned());
        let env: Vec<(String, String)> = self
            .authkey
            .map(|key| vec![("TS_AUTHKEY".to_string(), key)])
            .unwrap_or_default();
        run_agent("tailscale", &self.id, &action, &env)
    }
}

fn run_agent(
    family: &str,
    id: &str,
    action: &[String],
    env: &[(String, String)],
) -> Result<ExecResult> {
    let mut args = vec![family.to_string(), id.to_string()];
    args.extend(action.iter().cloned());
    let res = run_full(&args, env, None, 0)?;
    ExecResult {
        stdout: res.stdout,
        stderr: res.stderr,
        exit_code: res.exit_code,
        command: format!("{family} {}", action.join(" ")),
    }
    .ok_or_err()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn linux_minimal() {
        assert_eq!(
            Sandbox::linux("alpine").to_args(),
            s(&["linux", "alpine", "-d"])
        );
    }

    #[test]
    fn linux_full() {
        let args = Sandbox::linux("ghcr.io/owner/name:tag")
            .kernel("vmlinux")
            .kernel_version("6.6")
            .initramfs()
            .volume("web")
            .mount("~/project:/src")
            .mount("~/data:/data:ro")
            .entrypoint("/bin/sh")
            .console("hvc0")
            .port("8080:80")
            .forward(2222, 22)
            .network("devnet")
            .name("api")
            .cpus(2)
            .mem(1024)
            .command(["node", "server.js"])
            .to_args();
        assert_eq!(
            args,
            s(&[
                "linux",
                "ghcr.io/owner/name:tag",
                "-d",
                "--kernel",
                "vmlinux",
                "--kernel-version",
                "6.6",
                "--initramfs",
                "-v",
                "web",
                "--mount",
                "~/project:/src",
                "--mount",
                "~/data:/data:ro",
                "--entrypoint",
                "/bin/sh",
                "--console",
                "hvc0",
                "--port",
                "8080:80",
                "--port",
                "2222:22",
                "--network",
                "devnet",
                "--name",
                "api",
                "--cpus",
                "2",
                "--mem",
                "1024",
                "--",
                "node",
                "server.js",
            ])
        );
    }

    #[test]
    fn net_disabled_ordering() {
        // --no-net, then ports, then --mac, then --network.
        let args = Sandbox::linux("alpine")
            .no_net()
            .port("2222:22")
            .mac("de:ad:be:ef:00:01")
            .network("devnet")
            .to_args();
        assert_eq!(
            args,
            s(&[
                "linux",
                "alpine",
                "-d",
                "--no-net",
                "--port",
                "2222:22",
                "--mac",
                "de:ad:be:ef:00:01",
                "--network",
                "devnet",
            ])
        );
    }

    #[test]
    fn freebsd_full() {
        let args = Sandbox::freebsd()
            .version("14.3")
            .firmware("KRUN_EFI.fd")
            .force()
            .persist()
            .volume("db")
            .attach_disk("extra.raw")
            .attach_disk("ro.raw:ro")
            .mem(2048)
            .name("bsd")
            .to_args();
        assert_eq!(
            args,
            s(&[
                "freebsd",
                "-d",
                "--version",
                "14.3",
                "--firmware",
                "KRUN_EFI.fd",
                "--force",
                "--persist",
                "-v",
                "db",
                "--attach-disk",
                "extra.raw",
                "--attach-disk",
                "ro.raw:ro",
                "--name",
                "bsd",
                "--mem",
                "2048",
            ])
        );
    }

    #[test]
    fn netbsd_minimal() {
        assert_eq!(
            Sandbox::netbsd().version("10.1").volume("db").to_args(),
            s(&["netbsd", "-d", "--version", "10.1", "-v", "db"])
        );
    }

    #[test]
    fn firmware_positionals() {
        assert_eq!(
            Sandbox::firmware("KRUN_EFI.fd", "disk.raw").to_args(),
            s(&[
                "firmware",
                "--firmware",
                "KRUN_EFI.fd",
                "--disk",
                "disk.raw",
                "-d",
            ])
        );
    }

    #[test]
    fn kernel_initramfs_takes_a_path() {
        // For kernel, initramfs is a path (value), not a bare flag.
        let args = Sandbox::kernel("netbsd")
            .format("elf")
            .initramfs("initrd.img")
            .cmdline("root=ld0a")
            .disk("root.raw")
            .to_args();
        assert_eq!(
            args,
            s(&[
                "kernel",
                "--kernel",
                "netbsd",
                "-d",
                "--format",
                "elf",
                "--initramfs",
                "initrd.img",
                "--cmdline",
                "root=ld0a",
                "--disk",
                "root.raw",
            ])
        );
    }

    #[test]
    fn nanos_image_goes_last() {
        let args = Sandbox::nanos("hello")
            .cmdline("x=1")
            .persist()
            .mem(512)
            .to_args();
        assert_eq!(
            args,
            s(&[
                "nanos",
                "-d",
                "--cmdline",
                "x=1",
                "--persist",
                "--mem",
                "512",
                "hello",
            ])
        );
    }

    #[test]
    fn osv_image_goes_last() {
        let args = Sandbox::osv("loader.img")
            .disk("root.raw")
            .gic("v2")
            .to_args();
        assert_eq!(
            args,
            s(&[
                "osv",
                "-d",
                "--disk",
                "root.raw",
                "--gic",
                "v2",
                "loader.img",
            ])
        );
    }

    #[test]
    fn unikraft_defaults_path() {
        assert_eq!(
            Sandbox::unikraft(".").cmdline("helloworld").to_args(),
            s(&["unikraft", "-d", "--cmdline", "helloworld", "."])
        );
    }

    #[test]
    fn solo5_trailing_args_after_separator() {
        let args = Sandbox::solo5("dist/hello.hvt")
            .block("storage=disk.img")
            .args(["--ipv4=10.0.0.2/24"])
            .to_args();
        assert_eq!(
            args,
            s(&[
                "solo5",
                "-d",
                "--block",
                "storage=disk.img",
                "dist/hello.hvt",
                "--",
                "--ipv4=10.0.0.2/24",
            ])
        );
    }

    #[test]
    fn machine_id_lines_are_recognized() {
        assert!(looks_like_machine_id("fab8f81e4f91"));
        assert!(looks_like_machine_id("abc123"));
        assert!(!looks_like_machine_id("abc12")); // too short
        assert!(!looks_like_machine_id("pulling alpine:3.20"));
        assert!(!looks_like_machine_id("FAB8F81E4F91")); // ids are lowercase
        assert!(!looks_like_machine_id(""));
    }

    #[test]
    fn ssh_port_is_read_from_the_banner() {
        assert_eq!(
            parse_ssh_port("  connect with: ssh -p 2222 root@localhost\n"),
            Some(2222)
        );
        assert_eq!(parse_ssh_port("no banner here"), None);
    }
}
