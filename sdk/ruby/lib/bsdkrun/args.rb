# frozen_string_literal: true

module Bsdkrun
  # Builds the +bsdkrun+ argv (minus the binary and global flags) for a detached
  # +create+. Every path ends with +-d+ so +create+ yields a handle.
  #
  # Options are a Hash with Symbol keys mirroring the TypeScript SDK's
  # +CreateOptions+, discriminated on +:os+.
  module Args
    module_function

    # Networking flags shared by every guest kind.
    # @param net [Hash, nil]
    # @return [Array<String>]
    def net_args(net)
      a = []
      return a unless net

      a.push("--no-net") if net[:disabled]
      Array(net[:ports]).each do |p|
        a.push("--port", p.is_a?(String) ? p : "#{p[:host]}:#{p[:guest]}")
      end
      a.push("--mac", net[:mac]) if net[:mac]
      a.push("--network", net[:network]) if net[:network]
      a
    end

    # +--name+ flag if a name is set.
    # @param o [Hash]
    # @return [Array<String>]
    def name_args(o)
      o[:name] ? ["--name", o[:name].to_s] : []
    end

    # +--cpus+ / +--mem+ sizing flags.
    # @param o [Hash]
    # @return [Array<String>]
    def vm_args(o)
      a = []
      a.push("--cpus", o[:cpus].to_s) unless o[:cpus].nil?
      a.push("--mem", o[:mem].to_s) unless o[:mem].nil?
      a
    end

    # Disk-persistence flags shared by BSD / firmware / kernel guests.
    # @param o [Hash]
    # @return [Array<String>]
    def disk_args(o)
      a = []
      a.push("--persist") if o[:persist]
      a.push("-v", o[:volume]) if o[:volume]
      Array(o[:attach_disk]).each { |d| a.push("--attach-disk", d) }
      a
    end

    # Build the full detached +create+ argv for the given options.
    #
    # @param opts [Hash] create options, discriminated on +:os+.
    # @return [Array<String>]
    # @raise [ArgumentError] on an unknown +:os+.
    def build_create_args(opts)
      case opts[:os].to_s
      when "linux"    then linux_args(opts)
      when "freebsd"  then freebsd_args(opts)
      when "netbsd"   then netbsd_args(opts)
      when "firmware" then firmware_args(opts)
      when "kernel"   then kernel_args(opts)
      when "unikraft" then unikraft_args(opts)
      else raise ArgumentError, "unknown os: #{opts[:os].inspect}"
      end
    end

    # @!visibility private
    def linux_args(opts)
      a = ["linux", opts.fetch(:image).to_s, "-d"]
      a.push("--kernel", opts[:kernel]) if opts[:kernel]
      a.push("--kernel-version", opts[:kernel_version]) if opts[:kernel_version]
      a.push("--initramfs") if opts[:initramfs]
      a.push("-v", opts[:volume]) if opts[:volume]
      Array(opts[:mounts]).each { |m| a.push("--mount", m) }
      a.push("--entrypoint", opts[:entrypoint]) if opts[:entrypoint]
      a.push("--console", opts[:console]) if opts[:console]
      a.concat(net_args(opts[:net])).concat(name_args(opts)).concat(vm_args(opts))
      cmd = opts[:command]
      a.push("--", *cmd) if cmd && !cmd.empty?
      a
    end

    # @!visibility private
    def freebsd_args(opts)
      a = ["freebsd", "-d"]
      a.push("--version", opts[:version].to_s) if opts[:version]
      a.push("--firmware", opts[:firmware]) if opts[:firmware]
      a.push("--force") if opts[:force]
      a.concat(disk_args(opts)).concat(net_args(opts[:net]))
       .concat(name_args(opts)).concat(vm_args(opts))
    end

    # @!visibility private
    def netbsd_args(opts)
      a = ["netbsd", "-d"]
      a.push("--version", opts[:version].to_s) if opts[:version]
      a.push("--force") if opts[:force]
      a.concat(disk_args(opts)).concat(net_args(opts[:net]))
       .concat(name_args(opts)).concat(vm_args(opts))
    end

    # @!visibility private
    def firmware_args(opts)
      a = ["firmware", "--firmware", opts.fetch(:firmware), "--disk", opts.fetch(:disk), "-d"]
      a.concat(disk_args(opts)).concat(net_args(opts[:net]))
       .concat(name_args(opts)).concat(vm_args(opts))
    end

    # @!visibility private
    def kernel_args(opts)
      a = ["kernel", "--kernel", opts.fetch(:kernel), "-d"]
      a.push("--format", opts[:format].to_s) if opts[:format]
      a.push("--initramfs", opts[:initramfs]) if opts[:initramfs]
      a.push("--cmdline", opts[:cmdline]) if opts[:cmdline]
      a.push("--disk", opts[:disk]) if opts[:disk]
      a.concat(disk_args(opts)).concat(net_args(opts[:net]))
       .concat(name_args(opts)).concat(vm_args(opts))
    end

    # @!visibility private
    #
    # No +disk_args+: a unikernel has no disk, so there is nothing to persist,
    # attach or clone. +:path+ is a kraft project dir or an image; default +.+.
    def unikraft_args(opts)
      a = ["unikraft", "-d"]
      a.push("--cmdline", opts[:cmdline]) if opts[:cmdline]
      a.push("--initramfs", opts[:initramfs]) if opts[:initramfs]
      a.concat(net_args(opts[:net])).concat(name_args(opts)).concat(vm_args(opts))
      a.push((opts[:path] || ".").to_s)
    end
  end
end
