//! Build the `bsdkrun create` argv from typed options.
//!
//! This mirrors the Python SDK's `args.py` exactly, flag order included —
//! every path ends with `-d` (detached) so `create` yields a handle. The
//! required-field errors of the Python version mostly disappear here: the
//! per-kind builders in [`crate::sandbox`] take required fields as
//! constructor arguments, so an argv can only be built from a complete set.

/// The guest kinds `bsdkrun` can boot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Linux,
    Freebsd,
    Netbsd,
    Firmware,
    Kernel,
    Nanos,
    Osv,
    Unikraft,
    Solo5,
}

impl Kind {
    fn subcommand(self) -> &'static str {
        match self {
            Kind::Linux => "linux",
            Kind::Freebsd => "freebsd",
            Kind::Netbsd => "netbsd",
            Kind::Firmware => "firmware",
            Kind::Kernel => "kernel",
            Kind::Nanos => "nanos",
            Kind::Osv => "osv",
            Kind::Unikraft => "unikraft",
            Kind::Solo5 => "solo5",
        }
    }
}

/// Network options shared by every guest kind. `name` is only meaningful for
/// the remote GraphQL `NetInput`; the local CLI names machines with `--name`
/// instead.
#[derive(Debug, Default, Clone)]
pub(crate) struct NetOpts {
    pub no_net: bool,
    pub ports: Vec<String>,
    pub mac: Option<String>,
    pub network: Option<String>,
    pub name: Option<String>,
    /// Whether any setter ran — an untouched net maps to `null` over GraphQL,
    /// matching the Python SDK's `net=None`.
    pub touched: bool,
}

/// Every option any create builder can set. Irrelevant fields are simply
/// ignored by the kind's argv branch, exactly as Python ignores unexpected
/// kwargs — the per-kind builders are what keep users from setting them.
#[derive(Debug, Default, Clone)]
pub(crate) struct CreateOpts {
    pub image: Option<String>,
    pub path: Option<String>,
    pub version: Option<String>,
    pub firmware: Option<String>,
    pub disk: Option<String>,
    pub kernel: Option<String>,
    pub kernel_version: Option<String>,
    pub format: Option<String>,
    /// Linux `--initramfs` is a bare flag; kernel/unikraft take a path. Two
    /// fields so one builder's meaning can never leak into another's.
    pub initramfs_flag: bool,
    pub initramfs_path: Option<String>,
    pub cmdline: Option<String>,
    pub entrypoint: Option<String>,
    /// Guest environment for the entrypoint, as sorted `K=V` pairs.
    pub env: Vec<(String, String)>,
    pub console: Option<String>,
    pub gic: Option<String>,
    pub force: bool,
    pub persist: bool,
    pub volume: Option<String>,
    pub attach_disk: Vec<String>,
    pub mounts: Vec<String>,
    pub block: Vec<String>,
    pub command: Vec<String>,
    /// Solo5 guest args, passed after a literal `--`.
    pub trailing_args: Vec<String>,
    pub net: NetOpts,
    pub name: Option<String>,
    pub cpus: Option<u32>,
    pub mem: Option<u32>,
    pub log_level: Option<u32>,
}

/// `-e K=V` per entry, sorted by key, so the argv is deterministic regardless
/// of how the caller built the list. The guest sees the same environment either
/// way.
fn push_env(a: &mut Vec<String>, env: &[(String, String)]) {
    let mut pairs: Vec<&(String, String)> = env.iter().collect();
    pairs.sort_by(|x, y| x.0.cmp(&y.0));
    for (key, value) in pairs {
        a.push("-e".into());
        a.push(format!("{key}={value}"));
    }
}

fn push_opt(out: &mut Vec<String>, flag: &str, value: &Option<String>) {
    if let Some(v) = value {
        out.push(flag.to_string());
        out.push(v.clone());
    }
}

fn net_args(n: &NetOpts, out: &mut Vec<String>) {
    if n.no_net {
        out.push("--no-net".into());
    }
    for port in &n.ports {
        out.push("--port".into());
        out.push(port.clone());
    }
    push_opt(out, "--mac", &n.mac);
    push_opt(out, "--network", &n.network);
}

fn name_args(o: &CreateOpts, out: &mut Vec<String>) {
    push_opt(out, "--name", &o.name);
}

fn vm_args(o: &CreateOpts, out: &mut Vec<String>) {
    if let Some(cpus) = o.cpus {
        out.push("--cpus".into());
        out.push(cpus.to_string());
    }
    if let Some(mem) = o.mem {
        out.push("--mem".into());
        out.push(mem.to_string());
    }
}

fn disk_args(o: &CreateOpts, out: &mut Vec<String>) {
    if o.persist {
        out.push("--persist".into());
    }
    push_opt(out, "-v", &o.volume);
    for disk in &o.attach_disk {
        out.push("--attach-disk".into());
        out.push(disk.clone());
    }
}

fn common_tail(o: &CreateOpts, out: &mut Vec<String>) {
    net_args(&o.net, out);
    name_args(o, out);
    vm_args(o, out);
}

/// Build the full `bsdkrun` argv (minus the binary and global flags).
pub(crate) fn build_create_args(kind: Kind, o: &CreateOpts) -> Vec<String> {
    let mut a: Vec<String> = vec![kind.subcommand().to_string()];
    match kind {
        Kind::Linux => {
            a.push(o.image.clone().unwrap_or_default());
            a.push("-d".into());
            push_opt(&mut a, "--kernel", &o.kernel);
            push_opt(&mut a, "--kernel-version", &o.kernel_version);
            if o.initramfs_flag {
                a.push("--initramfs".into());
            }
            push_opt(&mut a, "-v", &o.volume);
            for mount in &o.mounts {
                a.push("--mount".into());
                a.push(mount.clone());
            }
            for disk in &o.attach_disk {
                a.push("--attach-disk".into());
                a.push(disk.clone());
            }
            push_opt(&mut a, "--entrypoint", &o.entrypoint);
            push_env(&mut a, &o.env);
            push_opt(&mut a, "--console", &o.console);
            common_tail(o, &mut a);
            if !o.command.is_empty() {
                a.push("--".into());
                a.extend(o.command.iter().cloned());
            }
        }
        Kind::Freebsd => {
            a.push("-d".into());
            push_opt(&mut a, "--version", &o.version);
            push_opt(&mut a, "--firmware", &o.firmware);
            if o.force {
                a.push("--force".into());
            }
            disk_args(o, &mut a);
            common_tail(o, &mut a);
        }
        Kind::Netbsd => {
            a.push("-d".into());
            push_opt(&mut a, "--version", &o.version);
            if o.force {
                a.push("--force".into());
            }
            disk_args(o, &mut a);
            common_tail(o, &mut a);
        }
        Kind::Firmware => {
            a.push("--firmware".into());
            a.push(o.firmware.clone().unwrap_or_default());
            a.push("--disk".into());
            a.push(o.disk.clone().unwrap_or_default());
            a.push("-d".into());
            disk_args(o, &mut a);
            common_tail(o, &mut a);
        }
        Kind::Kernel => {
            a.push("--kernel".into());
            a.push(o.kernel.clone().unwrap_or_default());
            a.push("-d".into());
            push_opt(&mut a, "--format", &o.format);
            push_opt(&mut a, "--initramfs", &o.initramfs_path);
            push_opt(&mut a, "--cmdline", &o.cmdline);
            push_opt(&mut a, "--disk", &o.disk);
            disk_args(o, &mut a);
            common_tail(o, &mut a);
        }
        Kind::Nanos => {
            a.push("-d".into());
            push_opt(&mut a, "--kernel", &o.kernel);
            push_opt(&mut a, "--cmdline", &o.cmdline);
            if o.persist {
                a.push("--persist".into());
            }
            common_tail(o, &mut a);
            a.push(o.image.clone().unwrap_or_default());
        }
        Kind::Osv => {
            // Like nanos: no agent, so no volume/repo/command. OSv does have a
            // root filesystem, so --persist applies.
            a.push("-d".into());
            push_opt(&mut a, "--cmdline", &o.cmdline);
            push_opt(&mut a, "--disk", &o.disk);
            push_opt(&mut a, "--gic", &o.gic);
            if o.persist {
                a.push("--persist".into());
            }
            common_tail(o, &mut a);
            a.push(o.image.clone().unwrap_or_default());
        }
        Kind::Unikraft => {
            // No disk_args: a unikernel has no disk, so there is nothing to
            // persist, attach or clone. Mounts are the exception — virtio-fs
            // shares, which need neither a disk nor an agent.
            a.push("-d".into());
            push_opt(&mut a, "--cmdline", &o.cmdline);
            push_opt(&mut a, "--initramfs", &o.initramfs_path);
            for mount in &o.mounts {
                a.push("--mount".into());
                a.push(mount.clone());
            }
            common_tail(o, &mut a);
            a.push(o.path.clone().unwrap_or_else(|| ".".into()));
        }
        Kind::Solo5 => {
            // Like unikraft, no disk_args — and not even mounts: a Solo5
            // unikernel declares its devices in its own MFT1 manifest, so only
            // the block backing files are passed. Guest args go last, after a
            // literal "--" — MirageOS options look like bsdkrun's own
            // (e.g. --ipv4=...), so the CLI takes them as trailing args.
            a.push("-d".into());
            for block in &o.block {
                a.push("--block".into());
                a.push(block.clone());
            }
            common_tail(o, &mut a);
            a.push(o.path.clone().unwrap_or_else(|| ".".into()));
            if !o.trailing_args.is_empty() {
                a.push("--".into());
                a.extend(o.trailing_args.iter().cloned());
            }
        }
    }
    a
}

pub(crate) fn strvec<I, S>(items: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    items.into_iter().map(Into::into).collect()
}
