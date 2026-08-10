(ns bsdkrun.sandbox-test
  "Only [[bsdkrun.sandbox/id]] and the `with-*` create-options builders are
  pure enough to unit test without spawning a real `bsdkrun` binary —
  everything else in that namespace shells out."
  (:require [clojure.test :refer [deftest is]]
            [bsdkrun.sandbox :as sandbox]))

(deftest id-from-sandbox-map
  (is (= "abc123" (sandbox/id {:id "abc123" :ssh-port 2222}))))

(deftest id-from-bare-string
  (is (= "web-1" (sandbox/id "web-1"))))

(deftest id-from-map-without-id
  (is (nil? (sandbox/id {:ssh-port 2222}))))

(deftest with-volume-sets-volume
  (is (= {:os :linux :image "alpine" :volume "web"}
         (sandbox/with-volume {:os :linux :image "alpine"} "web"))))

(deftest with-network-sets-nested-net-network
  (is (= {:os :linux :net {:network "devnet"}}
         (sandbox/with-network {:os :linux} "devnet"))))

(deftest with-network-preserves-other-net-keys
  (is (= {:net {:ports ["2222:22"] :mac "AA:BB" :network "devnet"}}
         (sandbox/with-network {:net {:ports ["2222:22"] :mac "AA:BB"}} "devnet"))))

(deftest with-volume-and-with-network-compose
  (is (= {:os :linux :image "alpine" :volume "web" :net {:network "devnet"}}
         (-> {:os :linux :image "alpine"}
             (sandbox/with-volume "web")
             (sandbox/with-network "devnet")))))
