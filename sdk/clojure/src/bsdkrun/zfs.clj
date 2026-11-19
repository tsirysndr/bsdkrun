(ns bsdkrun.zfs
  "ZFS volumes: one zpool per volume on a disk image, held by one machine at
  a time (Linux, FreeBSD, NetBSD). Branches are datasets cloned copy-on-write
  inside the pool (`zfs snapshot` + `zfs clone`), so a branch costs nothing
  until it diverges. Mirrors `sdk/ruby/lib/bsdkrun/zfs.rb`."
  (:refer-clojure :exclude [list])
  (:require [bsdkrun.process :as process]
            [bsdkrun.types :as types]
            [bsdkrun.util :as util]
            [clojure.string :as str]))

(defn- line [argv label]
  (str/trim (:stdout (process/run! argv {:label label}))))

(defn list
  "List ZFS volumes, with their branches and holder."
  []
  (let [res (process/run! ["zfs" "ls" "--json"] {:label "bsdkrun zfs ls"})]
    (mapv types/zfs-volume-info-from-row (types/read-json-rows (:stdout res)))))

(defn branches
  "The branches of one volume."
  [volume]
  (let [res (process/run! ["zfs" "ls" volume "--json"] {:label "bsdkrun zfs ls"})]
    (mapv types/zfs-branch-from-row (types/read-json-rows (:stdout res)))))

(defn create!
  "Create a volume: a sparse image that becomes a zpool with a `main` branch
  the first time a guest imports it. `{:size \"10G\"}` (the default)."
  ([name] (create! name {}))
  ([name {:keys [size]}]
   (line (cond-> ["zfs" "create" name] size (conj "--size" size)) "bsdkrun zfs create")))

(defn branch!
  "Fork a branch copy-on-write from `:from` (`main` by default). Runs in the
  machine holding the volume or in a short-lived FreeBSD helper, so it can
  take as long as a BSD boot. Returns the new `VOL@BRANCH`."
  ([volume name] (branch! volume name {}))
  ([volume name {:keys [from]}]
   (line (cond-> ["zfs" "branch" volume name] from (conj "--from" from)) "bsdkrun zfs branch")))

(defn attach!
  "Attach `VOL` or `VOL@BRANCH` to a machine at `:mount` (`/mnt/VOL`, or
  `/mnt/VOL-BRANCH`, by default). A pool is imported by one machine at a
  time; a stopped machine picks it up on start, a running holder mounts the
  branch live. Returns the CLI's one-line report."
  ([target machine] (attach! target machine {}))
  ([target machine {:keys [mount]}]
   (line (cond-> ["zfs" "attach" target machine] mount (conj "--mount" mount)) "bsdkrun zfs attach")))

(defn detach!
  "Detach one branch (`VOL@BRANCH`) or the whole volume (`VOL`) from a machine."
  [target machine]
  (line ["zfs" "detach" target machine] "bsdkrun zfs detach"))

(defn destroy!
  "Destroy branches (`VOL@BRANCH`) or whole volumes (`VOL`). `targets` is a
  string or a vector of strings; `{:force true}` destroys a volume a stopped
  machine still has attached."
  ([targets] (destroy! targets {}))
  ([targets {:keys [force]}]
   (let [argv (cond-> ["zfs" "destroy"] force (conj "--force"))
         argv (into argv (util/as-seq targets))]
     (process/run! argv {:label "bsdkrun zfs destroy"})
     nil)))
