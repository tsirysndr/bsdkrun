;; tools.build tasks for the bsdkrun Clojure SDK.
;;
;; Usage (from sdk/clojure):
;;   clj -T:build jar      # build target/bsdkrun-<version>.jar
;;   clj -T:build install  # install the jar to the local ~/.m2
;;   clj -T:build deploy   # deploy to Clojars (needs CLOJARS_USERNAME / CLOJARS_PASSWORD)
;;   clj -T:build clean    # remove target/
(ns build
  (:require [clojure.tools.build.api :as b]
            [deps-deploy.deps-deploy :as dd]))

(def lib 'io.github.tsirysndr/bsdkrun)
(def version "0.1.0")
(def class-dir "target/classes")
(def basis (delay (b/create-basis {:project "deps.edn"})))
(def jar-file (format "target/bsdkrun-%s.jar" version))

(defn clean [_]
  (b/delete {:path "target"}))

(defn jar
  "Build target/bsdkrun-<version>.jar, with a generated pom.xml."
  [_]
  (b/write-pom {:class-dir class-dir
                :lib lib
                :version version
                :basis @basis
                :src-dirs ["src"]
                :scm {:url "https://github.com/tsirysndr/bsdkrun"
                      :connection "scm:git:https://github.com/tsirysndr/bsdkrun.git"
                      :developerConnection "scm:git:ssh://git@github.com/tsirysndr/bsdkrun.git"
                      :tag (str "clojure-sdk-v" version)}})
  (b/copy-dir {:src-dirs ["src"] :target-dir class-dir})
  (b/jar {:class-dir class-dir :jar-file jar-file})
  (println "Built" jar-file))

(defn install
  "Build the jar and install it to the local ~/.m2 repository."
  [_]
  (jar nil)
  (b/install {:basis @basis
              :lib lib
              :version version
              :jar-file jar-file
              :class-dir class-dir})
  (println "Installed" lib version "to the local .m2"))

(defn deploy
  "Build the jar and deploy it to Clojars. Reads CLOJARS_USERNAME /
  CLOJARS_PASSWORD from the environment (deps-deploy's standard Clojars auth
  convention) — never pass credentials on the command line."
  [_]
  (jar nil)
  (dd/deploy {:installer :remote
              :artifact jar-file
              :pom-file (b/pom-path {:lib lib :class-dir class-dir})}))
