(ns bsdkrun.networks
  "Global-network operations — shared subnets where members reach each other
  by IP and by name. Mirrors `sdk/ruby/lib/bsdkrun/networks.rb`."
  (:refer-clojure :exclude [list])
  (:require [bsdkrun.process :as process]
            [bsdkrun.sandbox :as sandbox]
            [bsdkrun.types :as types]
            [bsdkrun.util :as util]))

(defn list
  "List global networks and their member counts."
  []
  (let [res (process/run! ["network" "ls" "--json"] {:label "bsdkrun network ls"})]
    (mapv types/network-info-from-row (types/read-json-rows (:stdout res)))))

(defn create!
  "Create a global network (starts its shared switch)."
  [name]
  (process/run! ["network" "create" name] {:label "bsdkrun network create"})
  nil)

(defn remove!
  "Remove one or more networks. `names` is a string or a vector of strings.
  `{:force true}` allows removal with running members."
  ([names] (remove! names {}))
  ([names {:keys [force]}]
   (let [argv (cond-> ["network" "rm"] force (conj "--force"))
         argv (into argv (util/as-seq names))]
     (process/run! argv {:label "bsdkrun network rm"})
     nil)))

(defn connect!
  "Join or switch a machine to a network. Applies on the machine's next
  start. `machine` is an id or name."
  [machine network]
  (process/run! ["network" "connect" machine network] {:label "bsdkrun network connect"})
  nil)

(defn disconnect!
  "Detach a machine from its network. Applies on its next start."
  [machine]
  (process/run! ["network" "disconnect" machine] {:label "bsdkrun network disconnect"})
  nil)

(defn sync!
  "Refresh members' `/etc/hosts` so peers resolve by name (notably NetBSD)."
  [network]
  (process/run! ["network" "sync" network] {:label "bsdkrun network sync"})
  nil)

(defn members
  "The machines currently attached to `network` (running or stopped)."
  [network]
  (filterv #(= (:network %) network) (sandbox/list {:all true})))
