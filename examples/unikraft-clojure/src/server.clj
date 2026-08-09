(ns server
  "The HTTP endpoint. Two routes, the same pair the other Unikraft examples in
  this repository serve: `/` greets, `/info` reports the runtime versions.

  The server is `com.sun.net.httpserver`, the one in the JDK, rather than Ring
  and Jetty. That is deliberate: it is what the upstream Java example uses, it
  lives in a single 200 KiB module (`jdk.httpserver`), and it needs no NIO
  selector -- the thread-per-request model is a better fit for a guest whose
  epoll support comes from a syscall shim than an event loop would be."
  (:gen-class)
  (:require [clojure.data.json :as json])
  (:import (com.sun.net.httpserver HttpExchange HttpHandler HttpServer)
           (java.net InetSocketAddress)
           (java.nio.charset StandardCharsets)
           (java.util.concurrent Executors)))

(def ^:private port 3000)

(defn- respond
  [^HttpExchange exchange status ^String content-type ^String body]
  (let [bytes (.getBytes body StandardCharsets/UTF_8)]
    (.add (.getResponseHeaders exchange) "Content-Type" content-type)
    (.sendResponseHeaders exchange status (alength bytes))
    (with-open [out (.getResponseBody exchange)]
      (.write out bytes))))

(defn- info []
  (json/write-str
   {:runtime "clojure"
    :clojure (clojure-version)
    :java (System/getProperty "java.version")
    :vm (System/getProperty "java.vm.name")}))

(defn- handle-request [^HttpExchange exchange]
  (case (.getPath (.getRequestURI exchange))
    "/" (respond exchange 200 "text/plain" "Hello from Clojure on Unikraft!\n")
    "/info" (respond exchange 200 "application/json" (info))
    (respond exchange 404 "text/plain" "not found\n")))

(defn -main [& _args]
  (let [server (HttpServer/create (InetSocketAddress. "0.0.0.0" port) 0)]
    ;; With no executor set, the JDK server runs every handler on the acceptor
    ;; thread, which serialises requests. A small fixed pool is enough: the
    ;; guest has one vCPU, and an unbounded pool would let a burst of requests
    ;; allocate threads the unikernel has no memory for.
    (.setExecutor server (Executors/newFixedThreadPool 4))
    (.createContext server "/"
                    (reify HttpHandler
                      (handle [_ exchange] (handle-request exchange))))
    (.start server)
    (println (str "Clojure listening on port " port))))
