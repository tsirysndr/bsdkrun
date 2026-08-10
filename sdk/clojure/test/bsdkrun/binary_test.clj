(ns bsdkrun.binary-test
  "Binary-resolution tests. The JVM has no supported, portable way to mutate
  System/getenv for the current process (unlike Ruby's ENV[...]= or
  Elixir/Erlang's peristent_term-backed override), so instead of trying to
  fake environment mutation, bsdkrun.binary/candidates and /resolve both
  accept an explicit opts map that overrides real host state key by key —
  we exercise the pure discovery logic through that, plus real temp
  directories on a fabricated PATH string for the `which` fallback. The
  public override-setter (set-override!/reset!) is also covered directly."
  (:require [clojure.java.io :as io]
            [clojure.test :refer [deftest is testing]]
            [bsdkrun.binary :as binary])
  (:import [java.io File]))

(defn- tmp-dir ^File []
  (let [f (File/createTempFile "bsdkrun-test" "")]
    (.delete f)
    (.mkdirs f)
    f))

(defn- make-executable! ^File [^File dir name]
  (let [f (io/file dir name)]
    (spit f "#!/bin/sh\necho hi\n")
    (.setExecutable f true)
    f))

(deftest candidates-priority-order
  (testing "an explicit override comes first"
    (is (= ["/explicit/bsdkrun"]
           (binary/candidates {:override "/explicit/bsdkrun" :bsdkrun-bin nil
                                :path "" :repo-root nil}))))
  (testing "BSDKRUN_BIN comes after the override"
    (is (= ["/explicit/bsdkrun" "/env/bsdkrun"]
           (binary/candidates {:override "/explicit/bsdkrun" :bsdkrun-bin "/env/bsdkrun"
                                :path "" :repo-root nil}))))
  (testing "PATH comes after both, and in-repo dev builds come last"
    (let [dir (tmp-dir)
          exe (make-executable! dir "bsdkrun")
          repo (tmp-dir)]
      (is (= [(.getPath exe)
              (.getPath (io/file repo "target" "release" "bsdkrun"))
              (.getPath (io/file repo "target" "debug" "bsdkrun"))]
             (binary/candidates {:override nil :bsdkrun-bin nil
                                  :path (.getPath dir) :repo-root repo}))))))

(deftest candidates-skips-absent-optional-sources
  (is (= [] (binary/candidates {:override nil :bsdkrun-bin nil :path "" :repo-root nil}))))

(deftest resolve-picks-first-existing-and-caches
  (binary/reset!)
  (let [dir (tmp-dir)
        exe (make-executable! dir "bsdkrun")
        opts {:override nil :bsdkrun-bin nil :path (.getPath dir) :repo-root nil}]
    (is (= (.getPath exe) (binary/resolve opts)))
    ;; Cached: a second call with different (even unusable) opts still
    ;; returns the memoized result.
    (is (= (.getPath exe) (binary/resolve {:override nil :bsdkrun-bin nil
                                            :path "" :repo-root nil}))))
  (binary/reset!))

(deftest resolve-uses-bare-bsdkrun-bin-via-path
  (binary/reset!)
  (let [dir (tmp-dir)
        exe (make-executable! dir "bsdkrun")]
    (is (= (.getPath exe)
           (binary/resolve {:override nil :bsdkrun-bin "bsdkrun"
                             :path (.getPath dir) :repo-root nil}))))
  (binary/reset!))

(deftest resolve-throws-when-nothing-found
  (binary/reset!)
  (is (thrown-with-msg?
       clojure.lang.ExceptionInfo #"could not find the \"bsdkrun\" binary"
       (binary/resolve {:override nil :bsdkrun-bin nil :path "" :repo-root nil})))
  (try
    (binary/resolve {:override nil :bsdkrun-bin nil :path "" :repo-root nil})
    (catch clojure.lang.ExceptionInfo e
      (is (= :binary-not-found (:bsdkrun/error (ex-data e))))
      (is (= [] (:searched (ex-data e))))))
  (binary/reset!))

(deftest set-override-and-reset
  (binary/reset!)
  (is (nil? (binary/override)))
  (let [dir (tmp-dir)
        exe (make-executable! dir "bsdkrun")]
    (binary/set-override! (.getPath exe))
    (is (= (.getPath exe) (binary/override)))
    (is (= (.getPath exe) (binary/resolve)))
    (binary/reset!)
    (is (nil? (binary/override)))
    (is (thrown? clojure.lang.ExceptionInfo
                 (binary/resolve {:override nil :bsdkrun-bin nil :path "" :repo-root nil})))
    (binary/reset!)))

(deftest real-defaults-do-not-throw-when-building-candidates
  ;; A smoke test for the real (no-arg) discovery path: it should compute a
  ;; candidate list without error even though we don't assert its contents
  ;; (they depend on the host running the suite).
  (binary/reset!)
  (is (vector? (binary/candidates)))
  (binary/reset!))
