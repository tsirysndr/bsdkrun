(ns bsdkrun.args
  "Builds the `bsdkrun` argv (minus the binary and global flags) for a
  detached `create`. Every path ends with `-d` so `create!` always yields a
  handle.

  Options are a plain map with keyword keys mirroring the other SDKs'
  create-options shape, discriminated on `:os`. Mirrors
  `sdk/ruby/lib/bsdkrun/args.rb` 1:1."
  (:require [bsdkrun.errors :as errors]
            [bsdkrun.util :as util]))

(defn- fetch!
  "Like Ruby's `Hash#fetch` — the required option key must be present (even
  if its value happens to be falsy), or `errors/missing-option` is thrown."
  [opts k]
  (if (contains? opts k)
    (get opts k)
    (throw (errors/missing-option k))))

(defn net-args
  "Networking flags shared by every guest kind."
  [net]
  (if-not net
    []
    (vec
     (concat
      (when (:disabled net) ["--no-net"])
      (mapcat (fn [p]
                ["--port" (if (string? p) p (str (:host p) ":" (:guest p)))])
              (util/as-seq (:ports net)))
      (when (:mac net) ["--mac" (:mac net)])
      (when (:network net) ["--network" (:network net)])))))

(defn name-args
  "`--name` flag if a name is set."
  [o]
  (if (:name o) ["--name" (str (:name o))] []))

(defn vm-args
  "`--cpus` / `--mem` sizing flags."
  [o]
  (vec
   (concat
    (when (some? (:cpus o)) ["--cpus" (str (:cpus o))])
    (when (some? (:mem o)) ["--mem" (str (:mem o))]))))

(defn disk-args
  "Disk-persistence flags shared by BSD / firmware / kernel guests."
  [o]
  (vec
   (concat
    (when (:persist o) ["--persist"])
    (when (:volume o) ["-v" (:volume o)])
    (mapcat (fn [d] ["--attach-disk" d]) (util/as-seq (:attach-disk o))))))

(defn linux-args
  [opts]
  (vec
   (concat
    ["linux" (str (fetch! opts :image)) "-d"]
    (when (:kernel opts) ["--kernel" (:kernel opts)])
    (when (:kernel-version opts) ["--kernel-version" (:kernel-version opts)])
    (when (:initramfs opts) ["--initramfs"])
    (when (:volume opts) ["-v" (:volume opts)])
    (mapcat (fn [m] ["--mount" m]) (util/as-seq (:mounts opts)))
    (when (:entrypoint opts) ["--entrypoint" (:entrypoint opts)])
    (when (:console opts) ["--console" (:console opts)])
    (net-args (:net opts))
    (name-args opts)
    (vm-args opts)
    (let [cmd (:command opts)]
      (when (seq cmd) (into ["--"] cmd))))))

(defn freebsd-args
  [opts]
  (vec
   (concat
    ["freebsd" "-d"]
    (when (:version opts) ["--version" (str (:version opts))])
    (when (:firmware opts) ["--firmware" (:firmware opts)])
    (when (:force opts) ["--force"])
    (disk-args opts)
    (net-args (:net opts))
    (name-args opts)
    (vm-args opts))))

(defn netbsd-args
  [opts]
  (vec
   (concat
    ["netbsd" "-d"]
    (when (:version opts) ["--version" (str (:version opts))])
    (when (:force opts) ["--force"])
    (disk-args opts)
    (net-args (:net opts))
    (name-args opts)
    (vm-args opts))))

(defn firmware-args
  [opts]
  (vec
   (concat
    ["firmware" "--firmware" (fetch! opts :firmware) "--disk" (fetch! opts :disk) "-d"]
    (disk-args opts)
    (net-args (:net opts))
    (name-args opts)
    (vm-args opts))))

(defn kernel-args
  [opts]
  (vec
   (concat
    ["kernel" "--kernel" (fetch! opts :kernel) "-d"]
    (when (:format opts) ["--format" (str (:format opts))])
    (when (:initramfs opts) ["--initramfs" (:initramfs opts)])
    (when (:cmdline opts) ["--cmdline" (:cmdline opts)])
    (when (:disk opts) ["--disk" (:disk opts)])
    (disk-args opts)
    (net-args (:net opts))
    (name-args opts)
    (vm-args opts))))

(defn nanos-args
  "Nanos: no agent (like unikraft), but it has a root disk, so `:persist` is
  honored. `:image` is a path or a `~/.ops/images` name."
  [opts]
  (vec
   (concat
    ["nanos" "-d"]
    (when (:kernel opts) ["--kernel" (:kernel opts)])
    (when (:cmdline opts) ["--cmdline" (:cmdline opts)])
    (when (:persist opts) ["--persist"])
    (net-args (:net opts))
    (name-args opts)
    (vm-args opts)
    [(str (fetch! opts :image))])))

(defn osv-args
  "OSv: like nanos, no agent (no exec/shell/snapshot), but it does have a
  root filesystem, so `:persist` is honored. `:image` is a loader.img, or on
  x86_64 the loader ELF plus a `:disk`."
  [opts]
  (vec
   (concat
    ["osv" "-d"]
    (when (:cmdline opts) ["--cmdline" (:cmdline opts)])
    (when (:disk opts) ["--disk" (:disk opts)])
    (when (:gic opts) ["--gic" (str (:gic opts))])
    (when (:persist opts) ["--persist"])
    (net-args (:net opts))
    (name-args opts)
    (vm-args opts)
    [(str (fetch! opts :image))])))

(defn unikraft-args
  "No `disk-args`: a unikernel has no disk, so there is nothing to persist,
  attach or clone. `:path` is a kraft project dir or an image; default `.`.
  Volumes are the exception to \"no disk options\": virtio-fs shares, which
  need neither a disk nor an agent."
  [opts]
  (vec
   (concat
    ["unikraft" "-d"]
    (when (:cmdline opts) ["--cmdline" (:cmdline opts)])
    (when (:initramfs opts) ["--initramfs" (:initramfs opts)])
    (mapcat (fn [m] ["--mount" m]) (util/as-seq (:mounts opts)))
    (net-args (:net opts))
    (name-args opts)
    (vm-args opts)
    [(str (or (:path opts) "."))])))

(defn build-create-args
  "Build the full detached `create` argv for the given options.

  `opts` is a map discriminated on `:os` (a string or keyword — `\"linux\"`,
  `\"freebsd\"`, `\"netbsd\"`, `\"firmware\"`, `\"kernel\"`, `\"unikraft\"`,
  `\"nanos\"`, `\"osv\"`).

  Throws `errors/unknown-os` on an unrecognized `:os`."
  [opts]
  (case (some-> (:os opts) name)
    "linux" (linux-args opts)
    "freebsd" (freebsd-args opts)
    "netbsd" (netbsd-args opts)
    "firmware" (firmware-args opts)
    "kernel" (kernel-args opts)
    "unikraft" (unikraft-args opts)
    "nanos" (nanos-args opts)
    "osv" (osv-args opts)
    (throw (errors/unknown-os (:os opts)))))
