(ns bsdkrun.ci-test
  "The YAML the builder emits is consumed by tangled's own workflow parser
  (inside `bsdkrun ci`), so these tests pin the emitted shape — a change here
  is a change to what spindle would receive."
  (:require [bsdkrun.ci :as ci]
            [clojure.string :as str]
            [clojure.test :refer [deftest is]]))

(deftest full-workflow-shape
  (let [y (-> (ci/workflow "test")
              (ci/on-push "main")
              (ci/on-pull-request "main" "develop")
              (ci/deps "clojure" "jdk21")
              (ci/deps-from "github:nix-community/fenix/abc123" "stable.default")
              (ci/env "CI_FROM" "sdk")
              (ci/clone-depth 100)
              (ci/step "deps" "clojure -P")
              (ci/step "test" "clojure -X:test" {"JAVA_OPTS" "-Xmx1g"})
              (ci/yaml))]
    (is (str/includes? y "  - event: [\"push\"]\n    branch: \"main\""))
    (is (str/includes? y "branch: [\"main\", \"develop\"]"))
    (is (str/includes? y "engine: nixery"))
    (is (str/includes? y "\"nixpkgs\":\n    - \"clojure\"\n    - \"jdk21\""))
    (is (str/includes? y "\"github:nix-community/fenix/abc123\":"))
    (is (str/includes? y "CI_FROM: \"sdk\""))
    (is (str/includes? y "depth: 100"))
    (is (str/includes? y "- name: \"deps\"\n    command: |\n      clojure -P"))
    (is (str/includes? y "environment:\n      JAVA_OPTS: \"-Xmx1g\""))))

(deftest block-unsafe-command-falls-back-to-json
  ;; Trailing spaces do not survive a literal block scalar; the emitter must
  ;; switch representation rather than silently altering the command.
  (let [y (-> (ci/workflow "edge")
              (ci/step "tricky" "echo 'a'  \necho b")
              (ci/yaml))]
    (is (str/includes? y "command: \"echo 'a'  \\necho b\""))))

(deftest file-name-suffix
  (is (= "build.yml" (ci/file-name (ci/workflow "build"))))
  (is (= "build.yaml" (ci/file-name (ci/workflow "build.yaml")))))
