(ns bsdkrun.filesystem
  "Files in a running sandbox.

  Every call goes through the guest's exec agent, so the sandbox has to be
  running — there is no offline write. Each function takes the machine id, in
  keeping with the rest of the SDK.

      (require '[bsdkrun.filesystem :as fs])

      (fs/write-file \"web\" \"/app/main.py\" \"print('hi')\")
      (fs/read-text \"web\" \"/app/out.json\")
      (fs/upload \"web\" \"./src\" \"/app/src\")
      (fs/download \"web\" \"/app/dist\" \"./dist\" {:recursive true})"
  (:require [clojure.string :as str]
            [bsdkrun.errors :as errors]
            [bsdkrun.process :as process])
  (:import [java.io File]))

(defn- check!
  "Throw unless the transfer succeeded. The CLI already explains these well;
  strip its `Error: ` prefix."
  [{:keys [exit-code stderr]} path]
  (when-not (zero? exit-code)
    (let [text (-> (or stderr "") str/trim (str/replace-first #"^Error:\s*" ""))
          message (if (str/blank? text)
                    (str "file transfer failed for " path)
                    text)]
      (throw (errors/file-transfer-failed message path)))))

(defn- ->bytes [data]
  (if (string? data) (.getBytes ^String data "UTF-8") data))

(defn write-file
  "Write `data` (a string or byte array) to `path` in the guest, creating
  parent directories. Returns nil."
  [id path data]
  (-> (process/run-binary ["cp" "-" (str id ":" path)] {:stdin (->bytes data)})
      (check! path))
  nil)

(defn read-file
  "Read `path` from the guest as a byte array."
  [id path]
  (let [res (process/run-binary ["cp" (str id ":" path) "-"])]
    (check! res path)
    (:stdout res)))

(defn read-text
  "Read `path` from the guest and decode it (UTF-8 by default)."
  ([id path] (read-text id path "UTF-8"))
  ([id path encoding]
   (String. ^bytes (read-file id path) ^String encoding)))

(defn upload
  "Copy a host file or directory into the guest.

  A directory's *contents* land in `remote-path`, so
  `(upload id \"./src\" \"/app/src\")` leaves the guest's `/app/src` holding
  what `./src` holds. Whether it recurses is decided by looking at the local
  path, so callers do not have to say which kind of thing it is. Returns nil."
  [id local-path remote-path]
  (let [file (File. ^String (str local-path))]
    (when-not (.exists file)
      (throw (errors/file-transfer-failed
              (str "cannot upload " local-path ": no such file or directory")
              (str local-path))))
    (-> (process/run (cond-> ["cp"]
                       (.isDirectory file) (conj "-r")
                       :always (into [(str local-path) (str id ":" remote-path)])))
        (check! (str local-path))))
  nil)

(defn download
  "Copy a file or directory out of the guest onto the host.

  Pass `{:recursive true}` for a directory; unlike [[upload]] it cannot be
  detected here, because the path lives in the guest and answering would cost
  an extra round trip on every call. Returns nil."
  ([id remote-path local-path] (download id remote-path local-path {}))
  ([id remote-path local-path {:keys [recursive]}]
   (-> (process/run (cond-> ["cp"]
                      recursive (conj "-r")
                      :always (into [(str id ":" remote-path) (str local-path)])))
       (check! remote-path))
   nil))
