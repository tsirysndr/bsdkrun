(ns bsdkrun.util
  "Small helpers shared across the SDK's namespaces.")

(defn as-seq
  "Coerce `x` into a sequence: `nil` -> `()`, a single non-sequential value ->
  a one-element seq, anything already sequential -> itself unchanged. Mirrors
  the reference Ruby SDK's use of `Array()` for optional/repeatable options
  (e.g. `:mounts`, `:attach-disk`, `:key`)."
  [x]
  (cond
    (nil? x) []
    (sequential? x) x
    :else [x]))
