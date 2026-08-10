(ns bsdkrun.client-test
  "Tests for `bsdkrun.client`, run with no real daemon and no network beyond
  loopback — everything is driven against `bsdkrun.support.fake-graphql-server`,
  a from-scratch HTTP+WS server built on the JDK only (see that namespace's
  docstring for why it exists instead of `com.sun.net.httpserver.HttpServer`
  alone).

  Organized the way `sdk/ruby/test/test_client_http.rb` /
  `test_ws_client.rb` / `test_client_exec.rb` split up the same coverage —
  URL normalization + `client-from-env`, HTTP transport error mapping, the
  ack-gating/pending-queue protocol state machine, and `exec!`'s 3-step
  sequencing — just as `testing` blocks within one namespace rather than one
  file per concern, since the fixture (the fake server) is shared."
  (:require [clojure.test :refer [deftest is testing]]
            [bsdkrun.client :as client]
            [bsdkrun.support.fake-graphql-server :as srv]))

(defn- with-server
  "Start a fake server for `f` (a 1-arg fn taking the server map), always
  stopping it afterward."
  [opts f]
  (let [server (srv/start opts)]
    (try (f server) (finally ((:stop! server))))))

(defn- ack-ws-handler
  "A `:ws-handler` that acks immediately and otherwise just calls `on-msg`
  (which receives `out` and every parsed message after the init/ack
  handshake)."
  [on-msg]
  (fn [out msg]
    (case (get msg "type")
      "connection_init" (srv/send-json! out {"type" "connection_ack"})
      (on-msg out msg))))

;; ---------------------------------------------------------------------------
;; URL normalization
;; ---------------------------------------------------------------------------

(deftest normalize-url-adds-scheme-and-suffix
  (is (= "http://localhost:50052/graphql" (client/normalize-url "localhost:50052"))))

(deftest normalize-url-strips-trailing-slashes
  (is (= "http://host:50052/graphql" (client/normalize-url "http://host:50052///"))))

(deftest normalize-url-does-not-double-append-graphql
  (is (= "https://host:50052/graphql" (client/normalize-url "https://host:50052/graphql/"))))

(deftest normalize-url-preserves-https
  (is (= "https://host/graphql" (client/normalize-url "https://host"))))

(deftest normalize-url-trims-whitespace
  (is (= "http://host/graphql" (client/normalize-url "  host  "))))

(deftest ws-url-derives-from-http
  (is (= "ws://host:50052/graphql/ws" (client/ws-url "http://host:50052/graphql")))
  (is (= "wss://host:50052/graphql/ws" (client/ws-url "https://host:50052/graphql"))))

;; ---------------------------------------------------------------------------
;; client-from-env
;; ---------------------------------------------------------------------------

(deftest from-env-throws-without-url
  (let [e (try (client/client-from-env {}) (catch clojure.lang.ExceptionInfo e e))]
    (is (some? e))
    (is (= :missing-config (:bsdkrun/error (ex-data e))))
    (is (re-find #"BSDKRUN_URL" (ex-message e)))))

(deftest from-env-throws-with-url-but-no-token
  (let [e (try (client/client-from-env {"BSDKRUN_URL" "http://host:50052"})
               (catch clojure.lang.ExceptionInfo e e))]
    (is (some? e))
    (is (= :missing-config (:bsdkrun/error (ex-data e))))
    (is (re-find #"BSDKRUN_TOKEN" (ex-message e)))))

(deftest from-env-builds-a-client
  (let [c (client/client-from-env {"BSDKRUN_URL" "host:50052" "BSDKRUN_TOKEN" "tok"})]
    (is (= "http://host:50052/graphql" (:url c)))
    (is (= "tok" (:token c)))))

;; ---------------------------------------------------------------------------
;; HTTP transport: request() error mapping
;; ---------------------------------------------------------------------------

(deftest request-returns-data-on-success
  (with-server
   {:http-handler (fn [_q _v _h] ["200 OK" {"data" {"machines" []}}])}
   (fn [server]
     (let [c (client/new-client {:url (:url server) :token "tok"})]
       (is (= {"machines" []} (client/request c "{ machines { id } }")))))))

(deftest request-sends-bearer-header-and-json-body
  (with-server
   {:http-handler (fn [q v h]
                     (is (= "Bearer tok" (get h "authorization")))
                     (is (= "application/json" (get h "content-type")))
                     (is (= "{ x }" q))
                     (is (= {"a" 1} v))
                     ["200 OK" {"data" {"ok" true}}])}
   (fn [server]
     (let [c (client/new-client {:url (:url server) :token "tok"})]
       (client/request c "{ x }" {:a 1})))))

(deftest request-401-raises-auth-error
  (with-server
   {:http-handler (fn [_q _v _h] ["401 Unauthorized" {"errors" [{"message" "nope"}]}])}
   (fn [server]
     (let [c (client/new-client {:url (:url server) :token "bad"})
           e (try (client/request c "{ x }") (catch clojure.lang.ExceptionInfo e e))]
       (is (= :auth-error (:bsdkrun/error (ex-data e))))))))

(deftest request-unauthenticated-extension-raises-auth-error
  (with-server
   {:http-handler (fn [_q _v _h]
                     ["200 OK" {"errors" [{"message" "bad token"
                                            "extensions" {"code" "UNAUTHENTICATED"}}]}])}
   (fn [server]
     (let [c (client/new-client {:url (:url server) :token "bad"})
           e (try (client/request c "{ x }") (catch clojure.lang.ExceptionInfo e e))]
       (is (= :auth-error (:bsdkrun/error (ex-data e))))
       (is (= "bad token" (ex-message e)))))))

(deftest request-other-graphql-error-raises-graphql-error-with-code
  (with-server
   {:http-handler (fn [_q _v _h]
                     ["200 OK" {"errors" [{"message" "bad argument"
                                            "extensions" {"code" "INVALID_ARGUMENT"}}]}])}
   (fn [server]
     (let [c (client/new-client {:url (:url server) :token "tok"})
           e (try (client/request c "{ x }") (catch clojure.lang.ExceptionInfo e e))]
       (is (= :graphql-error (:bsdkrun/error (ex-data e))))
       (is (= "bad argument" (ex-message e)))
       (is (= "INVALID_ARGUMENT" (:code (ex-data e))))))))

(deftest request-unreachable-daemon-raises-graphql-error
  (let [c (client/new-client {:url "http://127.0.0.1:1/graphql" :token "tok"})
        e (try (client/request c "{ x }") (catch clojure.lang.ExceptionInfo e e))]
    (is (= :graphql-error (:bsdkrun/error (ex-data e))))
    (is (re-find #"cannot reach the bsdkrun daemon at" (ex-message e)))
    (is (re-find #"http://127\.0\.0\.1:1/graphql" (ex-message e)))))

;; ---------------------------------------------------------------------------
;; typed methods: field-name wiring over HTTP
;; ---------------------------------------------------------------------------

(deftest list-machines-maps-graphql-machine-into-sandbox-info
  (let [machine {"id" "abc123" "name" "api" "image" "alpine" "kind" "linux"
                 "command" "sleep 300" "status" "running" "running" true
                 "exitCode" nil "pid" 42 "detached" true "cpus" 2 "mem" 512
                 "volume" nil "stateDir" "/s" "network" "devnet" "netIp" "10.0.0.2"
                 "createdAt" 1700000000 "finishedAt" nil
                 "ports" [{"bind" "127.0.0.1" "host" 8080 "guest" 80}]}]
    (with-server
     {:http-handler (fn [q v _h]
                       (is (re-find #"machines\(all:\$all\)" q))
                       (is (= {"all" true} v))
                       ["200 OK" {"data" {"machines" [machine]}}])}
     (fn [server]
       (let [c (client/new-client {:url (:url server) :token "tok"})
             list (client/list-machines c {:all true})]
         (is (= 1 (count list)))
         (let [info (first list)]
           (is (= "abc123" (:id info)))
           (is (= 42 (:pid info)))
           (is (= "running" (:status info)))
           (is (= 1700000000 (:created-at info)))
           (is (nil? (:finished-at info)))
           (is (= 1 (count (:ports info))))
           (is (= 8080 (:host (first (:ports info)))))))))))

(deftest get-machine-returns-nil-when-machine-is-null
  (with-server
   {:http-handler (fn [_q _v _h] ["200 OK" {"data" {"machine" nil}}])}
   (fn [server]
     (let [c (client/new-client {:url (:url server) :token "tok"})]
       (is (nil? (client/get-machine c "missing")))))))

(deftest run-linux-sends-camel-case-input-and-returns-id
  (with-server
   {:http-handler (fn [q v _h]
                     (is (re-find #"RunLinuxInput" q))
                     (let [input (get v "i")]
                       (is (= "alpine" (get input "image")))
                       (is (= "6.6" (get input "kernelVersion")))
                       (is (= false (get-in input ["net" "noNet"])))
                       (is (= ["8080:80"] (get-in input ["net" "ports"]))))
                     ["200 OK" {"data" {"runLinux" "deadbeef0001"}}])}
   (fn [server]
     (let [c (client/new-client {:url (:url server) :token "tok"})
           id (client/run-linux! c {:image "alpine" :kernel-version "6.6"
                                     :net {:ports ["8080:80"]}})]
       (is (= "deadbeef0001" id))))))

(deftest run-linux-requires-image
  (with-server
   {:http-handler (fn [_q _v _h] ["200 OK" {"data" {"runLinux" "x"}}])}
   (fn [server]
     (let [c (client/new-client {:url (:url server) :token "tok"})]
       (is (thrown? clojure.lang.ExceptionInfo (client/run-linux! c {})))))))

(deftest run-bsd-maps-os-enum-and-disk-fields
  (with-server
   {:http-handler (fn [_q v _h]
                     (let [input (get v "i")]
                       (is (= "NETBSD" (get input "os")))
                       (is (= ["a.raw"] (get input "attachDisk")))
                       (is (= "20G" (get input "diskSize"))))
                     ["200 OK" {"data" {"runBsd" "cafebabe0002"}}])}
   (fn [server]
     (let [c (client/new-client {:url (:url server) :token "tok"})
           id (client/run-bsd! c {:os "netbsd" :attach-disk ["a.raw"] :disk-size "20G"})]
       (is (= "cafebabe0002" id))))))

(deftest run-bsd-unknown-os-throws
  (with-server
   {:http-handler (fn [_q _v _h] ["200 OK" {"data" {"runBsd" "x"}}])}
   (fn [server]
     (let [c (client/new-client {:url (:url server) :token "tok"})]
       (is (thrown? clojure.lang.ExceptionInfo (client/run-bsd! c {:os "plan9"})))))))

(deftest stop-returns-command-result
  (with-server
   {:http-handler (fn [_q v _h]
                     (is (= "m1" (get v "id")))
                     ["200 OK" {"data" {"stopMachine" {"exitCode" 0 "stdout" "ok" "stderr" ""}}}])}
   (fn [server]
     (let [c (client/new-client {:url (:url server) :token "tok"})
           result (client/stop! c "m1")]
       (is (= 0 (:exit-code result)))
       (is (= "ok" (:stdout result)))))))

(deftest remove-accepts-a-single-id-or-a-collection
  (with-server
   {:http-handler (fn [_q v _h]
                     (is (= ["a" "b"] (get v "ids")))
                     (is (= true (get v "force")))
                     ["200 OK" {"data" {"removeMachines" {"exitCode" 0 "stdout" "" "stderr" ""}}}])}
   (fn [server]
     (let [c (client/new-client {:url (:url server) :token "tok"})]
       (client/remove! c ["a" "b"] {:force true})))))

(deftest logs-returns-string
  (with-server
   {:http-handler (fn [_q v _h]
                     (is (= true (get v "boot")))
                     ["200 OK" {"data" {"machineLogs" "boot log\n"}}])}
   (fn [server]
     (let [c (client/new-client {:url (:url server) :token "tok"})]
       (is (= "boot log\n" (client/logs c "m1" {:boot true})))))))

;; ---------------------------------------------------------------------------
;; websocket protocol: ack-gating / pending-queue state machine
;; ---------------------------------------------------------------------------

(deftest connection-init-carries-bearer-token-and-ack-unblocks-subscribe
  (let [received-init (promise)
        results (atom [])
        done (promise)]
    (with-server
     {:ws-handler (fn [out msg]
                     (case (get msg "type")
                       "connection_init"
                       (do (deliver received-init (get msg "payload"))
                           (srv/send-json! out {"type" "connection_ack"}))
                       "subscribe"
                       (do (srv/send-json! out {"type" "next" "id" (get msg "id")
                                                 "payload" {"data" {"hello" "world"}}})
                           (srv/send-json! out {"type" "complete" "id" (get msg "id")}))
                       nil))}
     (fn [server]
       (let [c (client/new-client {:url (:url server) :token "secret-token"})]
         (client/subscribe c "subscription{hello}" {}
                            {:on-next (fn [d] (swap! results conj [:next d]))
                             :on-complete (fn [] (deliver done true))})
         (is (= {"authorization" "Bearer secret-token"} (deref received-init 3000 :timeout)))
         (is (true? (deref done 3000 :timeout)))
         (is (= [[:next {"hello" "world"}]] @results)))))))

(deftest subscribe-before-ack-is-queued-and-flushed-on-ack
  (let [subscribe-received-at (promise)
        done (promise)]
    (with-server
     {:ws-handler (fn [out msg]
                     (case (get msg "type")
                       "connection_init"
                       ;; Delay the ack so a subscribe() called immediately
                       ;; after connecting is guaranteed to still be pending.
                       (future (Thread/sleep 150)
                               (srv/send-json! out {"type" "connection_ack"}))
                       "subscribe"
                       (do (deliver subscribe-received-at (System/currentTimeMillis))
                           (srv/send-json! out {"type" "complete" "id" (get msg "id")}))
                       nil))}
     (fn [server]
       (let [c (client/new-client {:url (:url server) :token "t"})
             started-at (System/currentTimeMillis)]
         (client/subscribe c "subscription{x}" {}
                            {:on-next (fn [_])
                             :on-complete (fn [] (deliver done true))})
         (is (true? (deref done 3000 :timeout)))
         (let [elapsed (- (deref subscribe-received-at 0 -1) started-at)]
           (is (>= elapsed 100)
               "the daemon should not see `subscribe` before it acked connection_init")))))))

(deftest error-message-routes-to-on-error-and-removes-the-subscription
  (let [errp (promise)]
    (with-server
     {:ws-handler (ack-ws-handler
                   (fn [out msg]
                     (when (= "subscribe" (get msg "type"))
                       (srv/send-json! out {"type" "error" "id" (get msg "id")
                                             "payload" [{"message" "boom"}]}))))}
     (fn [server]
       (let [c (client/new-client {:url (:url server) :token "t"})]
         (client/subscribe c "subscription{x}" {} {:on-next (fn [_]) :on-error (fn [e] (deliver errp e))})
         (let [e (deref errp 3000 :timeout)]
           (is (= :graphql-error (:bsdkrun/error (ex-data e))))
           (is (= "boom" (ex-message e)))))))))

(deftest ping-is-answered-with-pong
  (let [pong-received (promise)]
    (with-server
     {:ws-handler (fn [out msg]
                     (case (get msg "type")
                       "connection_init" (do (srv/send-json! out {"type" "connection_ack"})
                                              (srv/send-json! out {"type" "ping"}))
                       "pong" (deliver pong-received true)
                       nil))}
     (fn [server]
       (let [c (client/new-client {:url (:url server) :token "t"})]
         (client/subscribe c "subscription{x}" {} {:on-next (fn [_])})
         (is (true? (deref pong-received 3000 :timeout))))))))

(deftest unsubscribe-sends-complete
  (let [complete-received (promise)]
    (with-server
     {:ws-handler (ack-ws-handler
                   (fn [_out msg]
                     (when (= "complete" (get msg "type"))
                       (deliver complete-received (get msg "id")))))}
     (fn [server]
       (let [c (client/new-client {:url (:url server) :token "t"})
             unsub (client/subscribe c "subscription{x}" {} {:on-next (fn [_])})]
         (Thread/sleep 100) ; let connection_ack land so `subscribe` actually went out
         (unsub)
         (is (not= :timeout (deref complete-received 3000 :timeout))))))))

(deftest close-before-ack-delivers-auth-error
  (let [errp (promise)]
    (with-server
     {:ws-handler (fn [out msg]
                     (when (= "connection_init" (get msg "type"))
                       (srv/close-ws! out)))}
     (fn [server]
       (let [c (client/new-client {:url (:url server) :token "t"})]
         (client/subscribe c "subscription{x}" {} {:on-next (fn [_]) :on-error (fn [e] (deliver errp e))})
         (is (= :auth-error (:bsdkrun/error (ex-data (deref errp 3000 :timeout))))))))))

(deftest close-after-ack-delivers-generic-graphql-error
  (let [errp (promise)]
    (with-server
     {:ws-handler (ack-ws-handler
                   (fn [out msg]
                     (when (= "subscribe" (get msg "type"))
                       (srv/close-ws! out))))}
     (fn [server]
       (let [c (client/new-client {:url (:url server) :token "t"})]
         (client/subscribe c "subscription{x}" {} {:on-next (fn [_]) :on-error (fn [e] (deliver errp e))})
         (let [e (deref errp 3000 :timeout)]
           (is (= :graphql-error (:bsdkrun/error (ex-data e))))
           (is (not= :auth-error (:bsdkrun/error (ex-data e))))
           (is (re-find #"closed" (ex-message e)))))))))

;; ---------------------------------------------------------------------------
;; exec!: the 3-step sequencing (openShell -> subscribe -> closeShell)
;; ---------------------------------------------------------------------------

(deftest exec-runs-openshell-then-subscribes-then-closes-shell
  (let [call-order (atom [])]
    (with-server
     {:http-handler (fn [q _v _h]
                       (cond
                         (re-find #"openShell" q)
                         (do (swap! call-order conj :open-shell)
                             ["200 OK" {"data" {"openShell" {"id" "sess1"}}}])
                         (re-find #"closeShell" q)
                         (do (swap! call-order conj :close-shell)
                             ["200 OK" {"data" {"closeShell" true}}])
                         :else ["200 OK" {"data" {}}]))
      :ws-handler (ack-ws-handler
                   (fn [out msg]
                     (when (= "subscribe" (get msg "type"))
                       (swap! call-order conj :subscribe)
                       (srv/send-json! out {"type" "next" "id" (get msg "id")
                                             "payload" {"data" {"shellOutput" {"dataBase64" "aGVsbG8=" "exitCode" nil}}}})
                       (srv/send-json! out {"type" "next" "id" (get msg "id")
                                             "payload" {"data" {"shellOutput" {"dataBase64" nil "exitCode" 0}}}}))))}
     (fn [server]
       (let [c (client/new-client {:url (:url server) :token "t"})
             result (client/exec! c "m1" ["echo" "hello"])]
         (is (= 0 (:exit-code result)))
         (is (= "hello" (String. ^bytes (:output result) "UTF-8")))
         (is (= [:open-shell :subscribe :close-shell] @call-order)))))))

(deftest exec-swallows-close-shell-failure-and-still-returns-the-real-result
  (with-server
   {:http-handler (fn [q _v _h]
                     (cond
                       (re-find #"openShell" q) ["200 OK" {"data" {"openShell" {"id" "sess1"}}}]
                       (re-find #"closeShell" q) ["200 OK" {"errors" [{"message" "already gone"}]}]
                       :else ["200 OK" {"data" {}}]))
    :ws-handler (ack-ws-handler
                 (fn [out msg]
                   (when (= "subscribe" (get msg "type"))
                     (srv/send-json! out {"type" "next" "id" (get msg "id")
                                           "payload" {"data" {"shellOutput" {"dataBase64" nil "exitCode" 7}}}}))))}
   (fn [server]
     (let [c (client/new-client {:url (:url server) :token "t"})
           result (client/exec! c "m1" ["false"])]
       ;; closeShell failed server-side; exec!'s real result must not be masked.
       (is (= 7 (:exit-code result)))))))

(deftest exec-sends-env-as-k-equals-v-list
  (with-server
   {:http-handler (fn [q v _h]
                     (if (re-find #"openShell" q)
                       (do (is (= ["A=1" "B=2"] (get v "e")))
                           ["200 OK" {"data" {"openShell" {"id" "sess1"}}}])
                       ["200 OK" {"data" {}}]))
    :ws-handler (ack-ws-handler
                 (fn [out msg]
                   (when (= "subscribe" (get msg "type"))
                     (srv/send-json! out {"type" "next" "id" (get msg "id")
                                           "payload" {"data" {"shellOutput" {"dataBase64" nil "exitCode" 0}}}}))))}
   (fn [server]
     (let [c (client/new-client {:url (:url server) :token "t"})]
       (client/exec! c "m1" ["env"] {:env {:A 1 :B 2}})))))

;; ---------------------------------------------------------------------------
;; shell!: output/exit buffered until a callback is registered
;; ---------------------------------------------------------------------------

(deftest shell-buffers-output-and-exit-until-callbacks-are-registered
  (with-server
   {:http-handler (fn [q _v _h]
                     (cond
                       (re-find #"openShell" q) ["200 OK" {"data" {"openShell" {"id" "sess1"}}}]
                       (re-find #"closeShell" q) ["200 OK" {"data" {"closeShell" true}}]
                       :else ["200 OK" {"data" {}}]))
    :ws-handler (ack-ws-handler
                 (fn [out msg]
                   (when (= "subscribe" (get msg "type"))
                     (srv/send-json! out {"type" "next" "id" (get msg "id")
                                           "payload" {"data" {"shellOutput" {"dataBase64" "cHJvbXB0JCA=" "exitCode" nil}}}})
                     (srv/send-json! out {"type" "next" "id" (get msg "id")
                                           "payload" {"data" {"shellOutput" {"dataBase64" nil "exitCode" 0}}}}))))}
   (fn [server]
     (let [c (client/new-client {:url (:url server) :token "t"})
           handle (client/shell! c "m1")]
       ;; deliberately give the subscription time to deliver before any
       ;; callback is registered, exercising the buffering path.
       (Thread/sleep 200)
       (let [out (atom [])
             exitp (promise)]
         ((:on-output! handle) (fn [bytes] (swap! out conj (String. ^bytes bytes "UTF-8"))))
         ((:on-exit! handle) (fn [code] (deliver exitp code)))
         (is (= ["prompt$ "] @out))
         (is (= 0 (deref exitp 3000 :timeout))))
       ((:close! handle))))))
