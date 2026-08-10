(ns bsdkrun.volumes
  "Host-level volume operations. Mirrors `sdk/ruby/lib/bsdkrun/volumes.rb`."
  (:refer-clojure :exclude [list])
  (:require [bsdkrun.process :as process]
            [bsdkrun.types :as types]
            [bsdkrun.util :as util]))

(defn list
  "List persistent volumes."
  []
  (let [res (process/run! ["volume" "ls" "--json"] {:label "bsdkrun volume ls"})]
    (mapv types/volume-info-from-row (types/read-json-rows (:stdout res)))))

(defn remove!
  "Remove one or more volumes (and their data). `names` is a string or a
  vector of strings. `{:force true}` skips confirmation."
  ([names] (remove! names {}))
  ([names {:keys [force]}]
   (let [argv (cond-> ["volume" "rm"] force (conj "--force"))
         argv (into argv (util/as-seq names))]
     (process/run! argv {:label "bsdkrun volume rm"})
     nil)))
