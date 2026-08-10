(ns bsdkrun.images
  "Host-level image operations. Mirrors `sdk/ruby/lib/bsdkrun/images.rb`."
  (:refer-clojure :exclude [list])
  (:require [bsdkrun.process :as process]
            [bsdkrun.types :as types]))

(defn list
  "List downloaded images (pulled OCI images + fetched BSD images)."
  []
  (let [res (process/run! ["images" "--json"] {:label "bsdkrun images"})]
    (mapv types/image-info-from-row (types/read-json-rows (:stdout res)))))
