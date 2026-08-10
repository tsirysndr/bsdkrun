(ns bsdkrun.types-test
  (:require [clojure.test :refer [deftest is]]
            [bsdkrun.types :as types]))

(deftest sandbox-info-from-row-full
  (let [info (types/sandbox-info-from-row
              {"id" "abc123" "name" "api" "image" "alpine" "kind" "linux"
               "command" "sleep 300" "running" true "exit_code" nil "pid" 42
               "detached" true "cpus" 2 "mem" 512 "volume" nil
               "state_dir" "/s" "network" "devnet" "net_ip" "10.0.0.2"
               "created_at" 1700000000 "finished_at" nil
               "ports" [{"bind" "127.0.0.1" "host" 8080 "guest" 80}]})]
    (is (= "abc123" (:id info)))
    (is (= "api" (:name info)))
    (is (= "running" (:status info)))
    (is (true? (:running info)))
    (is (nil? (:exit-code info)))
    (is (= 42 (:pid info)))
    (is (true? (:detached info)))
    (is (= 2 (:cpus info)))
    (is (= 512 (:mem info)))
    (is (= "/s" (:state-dir info)))
    (is (= "devnet" (:network info)))
    (is (= "10.0.0.2" (:net-ip info)))
    (is (= 1700000000 (:created-at info)))
    (is (nil? (:finished-at info)))
    (is (= 1 (count (:ports info))))
    (is (= {:bind "127.0.0.1" :host 8080 :guest 80} (first (:ports info))))))

(deftest sandbox-info-from-row-sparse
  (let [info (types/sandbox-info-from-row {"id" "abc123"})]
    (is (= "abc123" (:id info)))
    (is (= "" (:image info)))
    (is (= "exited" (:status info)))
    (is (false? (:running info)))
    (is (nil? (:exit-code info)))
    (is (nil? (:pid info)))
    (is (= 0 (:cpus info)))
    (is (= 0 (:mem info)))
    (is (= [] (:ports info)))))

(deftest port-forward-from-row
  (is (= {:bind "127.0.0.1" :host 8080 :guest 80}
         (types/port-forward-from-row {"bind" "127.0.0.1" "host" 8080 "guest" 80}))))

(deftest image-info-from-row
  (is (= {:id "i1" :reference "alpine:3.19" :digest "sha256:x" :size 1024
          :rootfs "/r" :created-at 111}
         (types/image-info-from-row
          {"id" "i1" "reference" "alpine:3.19" "digest" "sha256:x"
           "size" 1024 "rootfs" "/r" "created_at" 111}))))

(deftest volume-info-from-row
  (is (= {:name "web" :guest nil :base nil :path "/v/web" :size "10G"
          :created-at 222 :tracked true}
         (types/volume-info-from-row
          {"name" "web" "path" "/v/web" "size" "10G" "created_at" 222
           "tracked" true}))))

(deftest volume-info-from-row-nil-created-at
  (is (nil? (:created-at (types/volume-info-from-row {"name" "web" "path" "/v/web"})))))

(deftest network-info-from-row
  (is (= {:name "devnet" :subnet "10.0.0.0/24" :gateway "10.0.0.1"
          :members 2 :running 1 :up true :created-at 333}
         (types/network-info-from-row
          {"name" "devnet" "subnet" "10.0.0.0/24" "gateway" "10.0.0.1"
           "members" 2 "running" 1 "up" true "created_at" 333}))))

(deftest network-info-from-row-defaults
  (let [info (types/network-info-from-row {"name" "devnet"})]
    (is (= 0 (:members info)))
    (is (= 0 (:running info)))
    (is (false? (:up info)))
    (is (nil? (:created-at info)))))

(deftest result-helpers
  (let [ok {:stdout "hello\n\n" :stderr "" :exit-code 0 :command "x"}
        bad {:stdout "" :stderr "boom" :exit-code 1 :command "x"}]
    (is (true? (types/ok? ok)))
    (is (= "hello" (types/text ok)))
    (is (false? (types/ok? bad)))
    (is (thrown? clojure.lang.ExceptionInfo (types/throw-if-failed! bad)))
    (is (= ok (types/throw-if-failed! ok)))))

(deftest lines-helper
  (is (= ["a" "b"] (types/lines {:stdout "a\n\nb\n"}))))

(deftest json-helper
  (is (= {"x" 1} (types/json {:stdout "{\"x\":1}"}))))

(deftest read-json-rows
  (is (= [] (types/read-json-rows "")))
  (is (= [] (types/read-json-rows nil)))
  (is (= [{"a" 1}] (types/read-json-rows "[{\"a\":1}]"))))

;; --- GraphQL decoders (bsdkrun.client) ----------------------------------

(deftest sandbox-info-from-graphql-full
  (let [info (types/sandbox-info-from-graphql
              {"id" "abc123" "name" "api" "image" "alpine" "kind" "linux"
               "command" "sleep 300" "status" "running" "running" true
               "exitCode" nil "pid" 42 "detached" true "cpus" 2 "mem" 512
               "volume" nil "stateDir" "/s" "network" "devnet" "netIp" "10.0.0.2"
               "createdAt" 1700000000 "finishedAt" nil
               "ports" [{"bind" "127.0.0.1" "host" 8080 "guest" 80}]})]
    (is (= "abc123" (:id info)))
    (is (= "running" (:status info)))
    (is (true? (:running info)))
    (is (nil? (:exit-code info)))
    (is (= 42 (:pid info)))
    (is (= "/s" (:state-dir info)))
    (is (= "10.0.0.2" (:net-ip info)))
    (is (= 1700000000 (:created-at info)))
    (is (nil? (:finished-at info)))
    (is (= {:bind "127.0.0.1" :host 8080 :guest 80} (first (:ports info))))))

(deftest sandbox-info-from-graphql-sparse
  (let [info (types/sandbox-info-from-graphql {"id" "abc123"})]
    (is (= "abc123" (:id info)))
    (is (= "" (:image info)))
    (is (false? (:running info)))
    (is (nil? (:exit-code info)))
    (is (= 0 (:cpus info)))
    (is (= [] (:ports info)))))

(deftest command-result-from-graphql-defaults-blank-strings
  (is (= {:exit-code 0 :stdout "ok" :stderr ""}
         (types/command-result-from-graphql {"exitCode" 0 "stdout" "ok" "stderr" nil}))))

(deftest shell-session-info-from-graphql
  (is (= {:id "s1" :machine-id "m1" :finished false :truncated true}
         (types/shell-session-info-from-graphql
          {"id" "s1" "machineId" "m1" "finished" false "truncated" true}))))
