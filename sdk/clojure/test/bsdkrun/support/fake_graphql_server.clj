(ns bsdkrun.support.fake-graphql-server
  "A from-scratch GraphQL-over-HTTP-and-websocket server for tests, built on
  a raw `java.net.ServerSocket` — no dependency beyond the JDK, mirroring
  `sdk/ruby/test/support/fake_graphql_server.rb`'s approach (its `WsClient`
  is likewise hand-rolled over raw sockets there; here the *client* under
  test is `java.net.http.WebSocket`, well-tested and RFC 6455-compliant, so
  this fake server only needs to be correct enough to drive it through the
  real protocol paths — not a full RFC 6455 implementation).

  One `ServerSocket` accepts everything; each connection is classified by
  the presence of an `Upgrade: websocket` header, exactly like the real
  `bsdkrund` (`POST /graphql` and `/graphql/ws` share one bind address).

  Header/request-line parsing reads one byte at a time off the raw
  `InputStream` (never a `BufferedReader`) specifically so it cannot read
  ahead past the blank line terminating the headers — a `BufferedReader`
  would risk swallowing the first bytes of a websocket frame that arrived
  hot on the socket's heels into its own internal buffer, silently
  corrupting the frame the caller reads next."
  (:require [clojure.data.json :as json]
            [clojure.string :as str])
  (:import (java.io ByteArrayOutputStream EOFException InputStream OutputStream)
           (java.net InetAddress ServerSocket Socket)
           (java.nio.charset StandardCharsets)
           (java.security MessageDigest)
           (java.util Base64)))

(def ^:private guid "258EAFA5-E914-47DA-95CA-C5AB0DC85B11")

;; ---- raw-socket line/byte reading (no buffering ahead) --------------------

(defn- read-line-raw
  "Read one CRLF- or LF-terminated line, byte by byte. nil at EOF with
  nothing read yet."
  [^InputStream in]
  (let [buf (ByteArrayOutputStream.)]
    (loop []
      (let [b (.read in)]
        (cond
          (neg? b) (when (pos? (.size buf)) (.toString buf "UTF-8"))
          (= b 10) (let [s (.toString buf "UTF-8")]
                     (if (str/ends-with? s "\r") (subs s 0 (dec (count s))) s))
          :else (do (.write buf b) (recur)))))))

(defn- read-fully
  ^bytes [^InputStream in n]
  (let [buf (byte-array n)]
    (loop [off 0]
      (when (< off n)
        (let [r (.read in buf off (- n off))]
          (when (neg? r) (throw (EOFException. "socket closed mid-frame")))
          (recur (+ off r)))))
    buf))

(defn- read-request
  "Returns `[request-line headers]`, headers a lower-cased-key map, or nil
  at EOF before anything was read."
  [in]
  (when-let [request-line (read-line-raw in)]
    [request-line
     (loop [headers {}]
       (let [line (read-line-raw in)]
         (if (or (nil? line) (= line ""))
           headers
           (let [idx (str/index-of line ":")]
             (recur (if idx
                      (assoc headers
                             (str/lower-case (str/trim (subs line 0 idx)))
                             (str/trim (subs line (inc idx))))
                      headers))))))]))

;; ---- RFC 6455 minimal frame codec (server side: writes unmasked, reads
;; masked client frames) -----------------------------------------------------

(defn- read-frame
  "Returns `{:opcode ... :payload <byte[]>}` for exactly one frame.
  Fragmented messages are unsupported — tests only ever exchange small JSON
  text frames, same assumption the Ruby fake server's WebSocketFrame makes."
  [^InputStream in]
  (let [b0b1 (read-fully in 2)
        b0 (bit-and (aget b0b1 0) 0xFF)
        b1 (bit-and (aget b0b1 1) 0xFF)
        opcode (bit-and b0 0x0F)
        masked? (not (zero? (bit-and b1 0x80)))
        len0 (bit-and b1 0x7F)
        len (cond
              (= len0 126) (let [b (read-fully in 2)]
                             (bit-or (bit-shift-left (bit-and (aget b 0) 0xFF) 8)
                                     (bit-and (aget b 1) 0xFF)))
              (= len0 127) (let [b (read-fully in 8)]
                             (reduce (fn [acc i] (bit-or (bit-shift-left acc 8) (bit-and (aget b i) 0xFF)))
                                     0 (range 8)))
              :else len0)
        mask-key (when masked? (read-fully in 4))
        payload (if (pos? len) (read-fully in len) (byte-array 0))]
    (when masked?
      (dotimes [i (alength payload)]
        (aset payload i (unchecked-byte (bit-xor (aget payload i) (aget ^bytes mask-key (mod i 4)))))))
    {:opcode opcode :payload payload}))

(defn- length-bytes
  [len]
  (cond
    (< len 126) [len]
    (< len 65536) (list* 126 [(bit-and (bit-shift-right len 8) 0xFF) (bit-and len 0xFF)])
    :else (list* 127 (for [i (range 7 -1 -1)] (bit-and (bit-shift-right len (* 8 i)) 0xFF)))))

(defn- write-frame
  [^OutputStream out opcode ^bytes payload]
  (.write out (byte-array [(unchecked-byte (bit-or 0x80 opcode))]))
  (.write out (byte-array (map unchecked-byte (length-bytes (alength payload)))))
  (.write out payload)
  (.flush out))

(defn send-json!
  "Send a websocket text frame (unmasked, as a server frame must be) on the
  raw `OutputStream` handed to a `:ws-handler`."
  [^OutputStream out obj]
  (write-frame out 0x1 (.getBytes (json/write-str obj) StandardCharsets/UTF_8)))

(defn close-ws!
  [^OutputStream out]
  (write-frame out 0x8 (byte-array 0)))

;; ---- connection handling ---------------------------------------------------

(defn- sha1-accept
  [key]
  (let [md (MessageDigest/getInstance "SHA-1")
        digest (.digest md (.getBytes (str key guid) StandardCharsets/UTF_8))]
    (.encodeToString (Base64/getEncoder) digest)))

(defn- handle-http
  [in ^OutputStream out headers http-handler]
  (let [len (Integer/parseInt (get headers "content-length" "0"))
        raw (if (pos? len) (String. (read-fully in len) StandardCharsets/UTF_8) "{}")
        payload (json/read-str raw)
        [status body] (http-handler (get payload "query") (or (get payload "variables") {}) headers)
        body-bytes (.getBytes (json/write-str body) StandardCharsets/UTF_8)]
    (.write out (.getBytes (str "HTTP/1.1 " status "\r\n"
                                 "content-type: application/json\r\n"
                                 "Content-Length: " (alength body-bytes) "\r\n"
                                 "Connection: close\r\n\r\n")
                            StandardCharsets/UTF_8))
    (.write out body-bytes)
    (.flush out)))

(defn- handle-ws
  [in ^OutputStream out headers ws-handler]
  (let [accept (sha1-accept (get headers "sec-websocket-key"))]
    (.write out (.getBytes (str "HTTP/1.1 101 Switching Protocols\r\n"
                                 "Upgrade: websocket\r\n"
                                 "Connection: Upgrade\r\n"
                                 "Sec-WebSocket-Accept: " accept "\r\n"
                                 "Sec-WebSocket-Protocol: graphql-transport-ws\r\n\r\n")
                            StandardCharsets/UTF_8))
    (.flush out)
    (try
      (loop []
        (let [{:keys [opcode payload]} (read-frame in)]
          (case opcode
            8 nil
            1 (do (when ws-handler
                    (ws-handler out (json/read-str (String. ^bytes payload StandardCharsets/UTF_8))))
                  (recur))
            (recur))))
      (catch Exception _ nil))))

(defn- handle-connection
  [^Socket sock http-handler ws-handler]
  (try
    (let [in (.getInputStream sock)
          out (.getOutputStream sock)]
      (when-let [[_request-line headers] (read-request in)]
        (if (= "websocket" (str/lower-case (get headers "upgrade" "")))
          (handle-ws in out headers ws-handler)
          (handle-http in out headers http-handler))))
    (catch Exception _ nil)
    (finally (try (.close sock) (catch Exception _ nil)))))

(defn start
  "Start a fake HTTP+WS GraphQL server on 127.0.0.1 with an OS-assigned
  port.

  `:http-handler` — `(fn [query variables headers] [status-line body-map])`,
    e.g. `[\"200 OK\" {\"data\" {...}}]`.
  `:ws-handler` — `(fn [out msg] ...)`, called for every parsed JSON text
    frame received after the handshake; `out` is the raw `OutputStream` to
    pass to [[send-json!]]/[[close-ws!]] for replies.

  Returns `{:port :url :stop!}`."
  [{:keys [http-handler ws-handler]}]
  (let [server (ServerSocket. 0 50 (InetAddress/getByName "127.0.0.1"))
        port (.getLocalPort server)
        running (atom true)
        threads (atom [])
        accept-thread
        (doto (Thread.
               ^Runnable
               (fn []
                 (while @running
                   (try
                     (let [sock (.accept server)
                           t (doto (Thread. ^Runnable (fn [] (handle-connection sock http-handler ws-handler)))
                               (.setDaemon true))]
                       (swap! threads conj t)
                       (.start t))
                     (catch Exception _ (reset! running false))))))
          (.setDaemon true))]
    (.start accept-thread)
    {:port port
     :url (str "http://127.0.0.1:" port "/graphql")
     :stop! (fn []
              (reset! running false)
              (try (.close server) (catch Exception _ nil)))}))
