(ns bsdkrun.sandbox-test
  "Only [[bsdkrun.sandbox/id]] is pure enough to unit test without spawning a
  real `bsdkrun` binary — everything else in that namespace shells out."
  (:require [clojure.test :refer [deftest is]]
            [bsdkrun.sandbox :as sandbox]))

(deftest id-from-sandbox-map
  (is (= "abc123" (sandbox/id {:id "abc123" :ssh-port 2222}))))

(deftest id-from-bare-string
  (is (= "web-1" (sandbox/id "web-1"))))

(deftest id-from-map-without-id
  (is (nil? (sandbox/id {:ssh-port 2222}))))
