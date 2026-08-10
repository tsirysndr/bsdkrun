(ns bsdkrun.system
  "Host-level toolchain / image operations. Mirrors
  `sdk/ruby/lib/bsdkrun/system.rb`."
  (:require [clojure.string :as str]
            [bsdkrun.process :as process]))

(defn probe
  "Sanity-check the toolchain: verify libkrun links and a context can be
  created/configured. Does not boot. Returns true on success."
  []
  (zero? (:exit-code (process/run ["probe"]))))

(defn fetch-image!
  "Download + prepare a BSD image ahead of time. `os` is \"freebsd\" or
  \"netbsd\". `opts`: `:version`, `:force` (re-download even if cached).
  Returns the command output."
  ([os] (fetch-image! os {}))
  ([os {:keys [version force]}]
   (let [argv (cond-> ["fetch" "--os" (str os)]
                version (into ["--version" (str version)])
                force (conj "--force"))]
     (:stdout (process/run! argv {:label "bsdkrun fetch"})))))

(defn versions
  "List the builds available to fetch for a BSD (`os` is \"freebsd\" or
  \"netbsd\"), one version string per non-empty output line."
  [os]
  (let [res (process/run! ["versions" "--os" (str os)] {:label "bsdkrun versions"})]
    (->> (str/split (:stdout res) #"\n")
         (map str/trim)
         (remove empty?))))

(defn grow-disk!
  "Grow a raw disk image (the guest expands its root FS on next boot)."
  [disk size]
  (process/run! ["grow" "--disk" disk "--size" size] {:label "bsdkrun grow"})
  nil)
