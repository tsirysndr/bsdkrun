//! Thin FFI bindings to libkrun's C ABI (v1.19.x).
//!
//! libkrun is itself written in Rust but exposes a stable C API. On macOS it
//! drives Hypervisor.framework. We only bind the subset needed to launch a
//! microVM from an external kernel or firmware plus virtio-blk disks.

use std::ffi::CString;
use std::os::raw::c_char;
use std::path::Path;

// Kernel image formats accepted by krun_set_kernel.
pub const KRUN_KERNEL_FORMAT_RAW: u32 = 0;
pub const KRUN_KERNEL_FORMAT_ELF: u32 = 1;
#[allow(dead_code)]
pub const KRUN_KERNEL_FORMAT_PE_GZ: u32 = 2;
#[allow(dead_code)]
pub const KRUN_KERNEL_FORMAT_IMAGE_BZ2: u32 = 3;
#[allow(dead_code)]
pub const KRUN_KERNEL_FORMAT_IMAGE_GZ: u32 = 4;
#[allow(dead_code)]
pub const KRUN_KERNEL_FORMAT_IMAGE_ZSTD: u32 = 5;

// virtio-net feature bits (see libkrun.h). `COMPAT_NET_FEATURES` is the set
// libkrun's own passt/gvproxy helpers enable — the safe baseline (checksum
// offload + TSO/UFO) that a userspace proxy like gvproxy negotiates.
const NET_FEATURE_CSUM: u32 = 1 << 0;
const NET_FEATURE_GUEST_CSUM: u32 = 1 << 1;
const NET_FEATURE_GUEST_TSO4: u32 = 1 << 7;
const NET_FEATURE_GUEST_UFO: u32 = 1 << 10;
const NET_FEATURE_HOST_TSO4: u32 = 1 << 11;
const NET_FEATURE_HOST_UFO: u32 = 1 << 14;
const COMPAT_NET_FEATURES: u32 = NET_FEATURE_CSUM
    | NET_FEATURE_GUEST_CSUM
    | NET_FEATURE_GUEST_TSO4
    | NET_FEATURE_GUEST_UFO
    | NET_FEATURE_HOST_TSO4
    | NET_FEATURE_HOST_UFO;

// Per-interface flags. `NET_FLAG_VFKIT` tells libkrun the unixgram peer speaks
// gvproxy's "vfkit" framing (the mode gvproxy's `-listen-vfkit` socket uses).
const NET_FLAG_VFKIT: u32 = 1 << 0;

#[link(name = "krun")]
extern "C" {
    fn krun_set_log_level(level: u32) -> i32;
    fn krun_create_ctx() -> i32;
    fn krun_free_ctx(ctx_id: u32) -> i32;
    fn krun_set_vm_config(ctx_id: u32, num_vcpus: u8, ram_mib: u32) -> i32;
    fn krun_add_disk(
        ctx_id: u32,
        block_id: *const c_char,
        disk_path: *const c_char,
        read_only: bool,
    ) -> i32;
    fn krun_set_kernel(
        ctx_id: u32,
        kernel_path: *const c_char,
        kernel_format: u32,
        initramfs: *const c_char,
        cmdline: *const c_char,
    ) -> i32;
    fn krun_set_firmware(ctx_id: u32, firmware_path: *const c_char) -> i32;
    fn krun_set_root(ctx_id: u32, root_path: *const c_char) -> i32;
    fn krun_set_workdir(ctx_id: u32, workdir_path: *const c_char) -> i32;
    fn krun_set_exec(
        ctx_id: u32,
        exec_path: *const c_char,
        argv: *const *const c_char,
        envp: *const *const c_char,
    ) -> i32;
    fn krun_add_net_unixgram(
        ctx_id: u32,
        c_path: *const c_char,
        fd: i32,
        c_mac: *const u8,
        features: u32,
        flags: u32,
    ) -> i32;
    fn krun_disable_implicit_console(ctx_id: u32) -> i32;
    fn krun_add_serial_console_default(ctx_id: u32, input_fd: i32, output_fd: i32) -> i32;
    fn krun_start_enter(ctx_id: u32) -> i32;
}

/// Turn a negative libkrun return (a `-errno`) into a readable error.
fn check(ret: i32, what: &str) -> anyhow::Result<i32> {
    if ret < 0 {
        let errno = -ret;
        let msg = std::io::Error::from_raw_os_error(errno);
        anyhow::bail!("{what} failed: {msg} (errno {errno})");
    }
    Ok(ret)
}

fn cstr(p: &str) -> anyhow::Result<CString> {
    Ok(CString::new(p)?)
}

fn path_cstr(p: &Path) -> anyhow::Result<CString> {
    let s = p
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("path is not valid UTF-8: {}", p.display()))?;
    cstr(s)
}

/// Verify a libkrun symbol is actually resolvable in the loaded dylib before we
/// call it. Guards against DYLD loading an old libkrun that lacks newer symbols
/// (calling a missing symbol jumps through a NULL stub → SIGSEGV).
fn require_symbol(name: &str) -> anyhow::Result<()> {
    let c = CString::new(name)?;
    // RTLD_DEFAULT searches all loaded images; NULL => symbol not present.
    let found = unsafe { !libc::dlsym(libc::RTLD_DEFAULT, c.as_ptr()).is_null() };
    if !found {
        anyhow::bail!(
            "the loaded libkrun is missing `{name}`, so it's too old for bsdkrun.\n\
             This usually means DYLD is loading a stale libkrun ahead of Homebrew's — \
             check DYLD_LIBRARY_PATH (a common culprit is an old copy in ~/.local/lib).\n\
             Fix: update or remove that libkrun, or run with `env -u DYLD_LIBRARY_PATH bsdkrun …`."
        );
    }
    Ok(())
}

/// Pick a *pollable* fd to use as the serial console's input.
///
/// If stdin is a TTY we use it directly, so interactive keystrokes reach the
/// guest. Otherwise we return the read end of a fresh pipe: kqueue can poll it,
/// but it never yields data (nobody holds the write end open to send any). This
/// keeps non-interactive/captured runs from aborting inside libkrun.
fn console_input_fd() -> anyhow::Result<i32> {
    // STDIN_FILENO == 0.
    if unsafe { libc::isatty(0) } == 1 {
        tracing::debug!("console input: stdin is a tty, using fd 0 (interactive)");
        return Ok(0);
    }
    let mut fds = [0i32; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        anyhow::bail!(
            "creating console input pipe: {}",
            std::io::Error::last_os_error()
        );
    }
    // Deliberately leak the write end so the read end stays valid (never EOFs)
    // for the lifetime of the process/VM.
    tracing::debug!(
        read_fd = fds[0],
        "console input: stdin is not a tty, using a pipe (no interactive input)"
    );
    Ok(fds[0])
}

/// A libkrun configuration context. Freed on drop unless consumed by `start_enter`.
pub struct Ctx {
    id: u32,
    entered: bool,
}

impl Ctx {
    pub fn new() -> anyhow::Result<Self> {
        let id = unsafe { krun_create_ctx() };
        let id = check(id, "krun_create_ctx")?;
        Ok(Ctx {
            id: id as u32,
            entered: false,
        })
    }

    pub fn set_log_level(level: u32) -> anyhow::Result<()> {
        check(unsafe { krun_set_log_level(level) }, "krun_set_log_level")?;
        Ok(())
    }

    pub fn set_vm_config(&self, num_vcpus: u8, ram_mib: u32) -> anyhow::Result<()> {
        check(
            unsafe { krun_set_vm_config(self.id, num_vcpus, ram_mib) },
            "krun_set_vm_config",
        )?;
        Ok(())
    }

    pub fn add_disk(
        &self,
        block_id: &str,
        disk_path: &Path,
        read_only: bool,
    ) -> anyhow::Result<()> {
        let id = cstr(block_id)?;
        let path = path_cstr(disk_path)?;
        check(
            unsafe { krun_add_disk(self.id, id.as_ptr(), path.as_ptr(), read_only) },
            "krun_add_disk",
        )?;
        Ok(())
    }

    pub fn set_kernel(
        &self,
        kernel_path: &Path,
        format: u32,
        initramfs: Option<&Path>,
        cmdline: &str,
    ) -> anyhow::Result<()> {
        let kernel = path_cstr(kernel_path)?;
        let cmdline = cstr(cmdline)?;
        let initramfs = initramfs.map(path_cstr).transpose()?;
        let initramfs_ptr = initramfs
            .as_ref()
            .map(|c| c.as_ptr())
            .unwrap_or(std::ptr::null());
        check(
            unsafe {
                krun_set_kernel(
                    self.id,
                    kernel.as_ptr(),
                    format,
                    initramfs_ptr,
                    cmdline.as_ptr(),
                )
            },
            "krun_set_kernel",
        )?;
        Ok(())
    }

    pub fn set_firmware(&self, firmware_path: &Path) -> anyhow::Result<()> {
        let fw = path_cstr(firmware_path)?;
        check(
            unsafe { krun_set_firmware(self.id, fw.as_ptr()) },
            "krun_set_firmware",
        )?;
        Ok(())
    }

    /// Use a host directory as the guest's root filesystem, shared over
    /// virtio-fs. Requires a guest kernel built with `CONFIG_VIRTIO_FS=y`.
    /// libkrun injects its own init into this virtiofs root and runs the
    /// executable configured via [`Ctx::set_exec`].
    pub fn set_root(&self, root_path: &Path) -> anyhow::Result<()> {
        let path = path_cstr(root_path)?;
        check(
            unsafe { krun_set_root(self.id, path.as_ptr()) },
            "krun_set_root",
        )?;
        Ok(())
    }

    /// Set the working directory (relative to the virtio-fs root) for the
    /// executable started by libkrun's init.
    pub fn set_workdir(&self, workdir: &str) -> anyhow::Result<()> {
        let w = cstr(workdir)?;
        check(
            unsafe { krun_set_workdir(self.id, w.as_ptr()) },
            "krun_set_workdir",
        )?;
        Ok(())
    }

    /// Set the executable (relative to the virtio-fs root), its arguments, and
    /// its environment, for libkrun's init to exec as the guest's entrypoint.
    pub fn set_exec(
        &self,
        exec_path: &str,
        argv: &[String],
        envp: &[String],
    ) -> anyhow::Result<()> {
        let exec = cstr(exec_path)?;
        // argv/envp are NULL-terminated arrays of C string pointers. Keep the
        // owning CStrings alive until after the call.
        let argv_c: Vec<CString> = argv
            .iter()
            .map(|s| cstr(s))
            .collect::<anyhow::Result<_>>()?;
        let envp_c: Vec<CString> = envp
            .iter()
            .map(|s| cstr(s))
            .collect::<anyhow::Result<_>>()?;
        let mut argv_p: Vec<*const c_char> = argv_c.iter().map(|c| c.as_ptr()).collect();
        argv_p.push(std::ptr::null());
        let mut envp_p: Vec<*const c_char> = envp_c.iter().map(|c| c.as_ptr()).collect();
        envp_p.push(std::ptr::null());
        check(
            unsafe { krun_set_exec(self.id, exec.as_ptr(), argv_p.as_ptr(), envp_p.as_ptr()) },
            "krun_set_exec",
        )?;
        Ok(())
    }

    /// Add a virtio-net device backed by a gvproxy "vfkit" unixgram socket.
    ///
    /// libkrun's default networking is the TSI backend, which impersonates the
    /// guest's sockets from a shim *inside a Linux guest kernel* — BSD guests
    /// have no such shim, so TSI gives them no network at all. Instead we point
    /// libkrun at a userspace network stack (gvproxy) over a datagram socket;
    /// the guest then sees an ordinary virtio-net NIC it can DHCP on. The socket
    /// is created and served by [`crate::net::Gvproxy`].
    ///
    /// Must be called before `start_enter`. `mac` is the six-byte hardware
    /// address advertised to the guest.
    pub fn add_net_gvproxy(&self, vfkit_socket: &Path, mac: [u8; 6]) -> anyhow::Result<()> {
        // Added in the same libkrun era as the explicit-console API; guard so a
        // stale DYLD-loaded libkrun fails loudly instead of jumping a NULL stub.
        require_symbol("krun_add_net_unixgram")?;

        let path = path_cstr(vfkit_socket)?;
        check(
            unsafe {
                krun_add_net_unixgram(
                    self.id,
                    path.as_ptr(),
                    -1, // -1 => connect by path rather than a pre-opened fd
                    mac.as_ptr(),
                    COMPAT_NET_FEATURES,
                    NET_FLAG_VFKIT,
                )
            },
            "krun_add_net_unixgram",
        )?;
        Ok(())
    }

    /// Route the guest's legacy serial console (ttyS0) to this process's
    /// stdin/stdout.
    ///
    /// On aarch64/macOS libkrun creates an *implicit* console that is not the
    /// legacy PL011 UART the EDK2 firmware and BSD EFI loaders actually write
    /// to — so their output (the whole firmware banner + loader menu) never
    /// reaches our stdout. We disable the implicit console and add an explicit
    /// serial console instead, which then occupies ttyS0 (the PL011 the
    /// firmware drives) and is wired to stdout plus a pollable input fd.
    ///
    /// libkrun registers the console *input* fd with `kqueue`, which rejects
    /// non-pollable fds (regular files, `/dev/null`) — a bare
    /// `krun_add_serial_console_default(0, 1)` therefore aborts the whole
    /// process whenever stdin isn't a TTY (output redirected to a file, run
    /// non-interactively, launched via a shell's `!`/`&`, ...). To stay robust
    /// we only use fd 0 when it's actually a terminal; otherwise we hand
    /// libkrun the read end of a fresh pipe — a valid pollable fd that simply
    /// never delivers input (there's no interactive user to type anyway).
    pub fn attach_stdio_serial_console(&self) -> anyhow::Result<()> {
        // These two symbols were added in newer libkrun. If DYLD loaded an
        // older libkrun (e.g. via DYLD_LIBRARY_PATH pointing at a stale copy in
        // ~/.local/lib), calling them jumps through a NULL stub and segfaults.
        // Fail with an actionable message instead.
        for sym in [
            "krun_disable_implicit_console",
            "krun_add_serial_console_default",
        ] {
            require_symbol(sym)?;
        }

        check(
            unsafe { krun_disable_implicit_console(self.id) },
            "krun_disable_implicit_console",
        )?;

        let input_fd = console_input_fd()?;
        check(
            unsafe { krun_add_serial_console_default(self.id, input_fd, 1) },
            "krun_add_serial_console_default",
        )?;
        Ok(())
    }

    /// Boot the microVM. On success this blocks until the guest shuts down,
    /// then returns the guest's exit code.
    pub fn start_enter(mut self) -> anyhow::Result<i32> {
        self.entered = true;
        let ret = unsafe { krun_start_enter(self.id) };
        check(ret, "krun_start_enter")
    }
}

impl Drop for Ctx {
    fn drop(&mut self) {
        if !self.entered {
            unsafe {
                krun_free_ctx(self.id);
            }
        }
    }
}
