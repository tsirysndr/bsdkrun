(ns build
  "`clojure -T:build uber` -> target/server.jar, one file for the rootfs.

  An uberjar rather than a classpath of jars because the guest's root
  filesystem is a cpio archive unpacked into a RAM filesystem: fewer, larger
  files cost less than many small ones, and `java -jar` needs no `-cp` on the
  kernel command line, which is length-limited."
  (:require [clojure.tools.build.api :as b]))

(def ^:private class-dir "target/classes")
(def ^:private uber-file "target/server.jar")
(def ^:private basis (delay (b/create-basis {:project "deps.edn"})))

(defn uber [_]
  (b/delete {:path "target"})
  ;; AOT, not source. Without it the JVM compiles server.clj -- and every
  ;; namespace it requires -- on each boot, which is most of what people mean
  ;; by "Clojure starts slowly". `:direct-linking true` goes further and turns
  ;; var dereferences into static calls; both are safe here because nothing is
  ;; redefined at runtime.
  (b/compile-clj {:basis @basis
                  :src-dirs ["src"]
                  :class-dir class-dir
                  :ns-compile '[server]
                  :compile-opts {:direct-linking true}})
  (b/uber {:basis @basis
           :class-dir class-dir
           :uber-file uber-file
           :main 'server}))
