(ns bsdkrun.args-test
  "Unit tests for the argv builder — no process spawned, no binary needed."
  (:require [clojure.test :refer [deftest is testing]]
            [bsdkrun.args :as args]))

(defn- build [opts] (args/build-create-args opts))

(deftest linux-minimal
  (is (= ["linux" "alpine" "-d"] (build {:os "linux" :image "alpine"}))))

(deftest linux-full-ordering
  (let [got (build {:os "linux"
                     :image "ghcr.io/o/n:tag"
                     :kernel "vmlinux"
                     :kernel-version "6.6"
                     :initramfs true
                     :volume "web"
                     :mounts ["~/a:/a" "~/b:/b:ro"]
                     :entrypoint "/init"
                     :console "hvc0"
                     :net {:ports ["8080:80" {:host 2222 :guest 22}]
                           :mac "AA:BB"
                           :network "devnet"}
                     :name "api"
                     :cpus 2
                     :mem 1024
                     :command ["node" "server.js"]})
        expected ["linux" "ghcr.io/o/n:tag" "-d"
                  "--kernel" "vmlinux"
                  "--kernel-version" "6.6"
                  "--initramfs"
                  "-v" "web"
                  "--mount" "~/a:/a" "--mount" "~/b:/b:ro"
                  "--entrypoint" "/init"
                  "--console" "hvc0"
                  "--port" "8080:80" "--port" "2222:22"
                  "--mac" "AA:BB"
                  "--network" "devnet"
                  "--name" "api"
                  "--cpus" "2"
                  "--mem" "1024"
                  "--" "node" "server.js"]]
    (is (= expected got))))

(deftest linux-requires-image
  (is (thrown? clojure.lang.ExceptionInfo (build {:os "linux"}))))

(deftest freebsd
  (is (= ["freebsd" "-d"
          "--version" "14.3"
          "--firmware" "KRUN_EFI.fd"
          "--force"
          "--persist"
          "-v" "db"
          "--mem" "2048"]
         (build {:os "freebsd" :version "14.3" :firmware "KRUN_EFI.fd"
                 :force true :persist true :volume "db" :mem 2048}))))

(deftest netbsd-with-attach-disks
  (is (= ["netbsd" "-d"
          "--version" "10.1"
          "--attach-disk" "a.raw" "--attach-disk" "b.raw:ro"
          "--cpus" "4"]
         (build {:os "netbsd" :version "10.1"
                 :attach-disk ["a.raw" "b.raw:ro"] :cpus 4}))))

(deftest firmware
  (is (= ["firmware" "--firmware" "KRUN_EFI.fd" "--disk" "disk.raw" "-d"
          "--no-net"]
         (build {:os "firmware" :firmware "KRUN_EFI.fd" :disk "disk.raw"
                 :net {:disabled true}}))))

(deftest firmware-requires-firmware-and-disk
  (is (thrown? clojure.lang.ExceptionInfo (build {:os "firmware" :disk "disk.raw"})))
  (is (thrown? clojure.lang.ExceptionInfo (build {:os "firmware" :firmware "f.fd"}))))

(deftest kernel
  (is (= ["kernel" "--kernel" "netbsd" "-d"
          "--format" "elf"
          "--initramfs" "initrd.img"
          "--cmdline" "root=ld0a"
          "--disk" "root.raw"]
         (build {:os "kernel" :kernel "netbsd" :format "elf"
                 :initramfs "initrd.img" :cmdline "root=ld0a" :disk "root.raw"}))))

(deftest nanos
  (is (= ["nanos" "-d" "--kernel" "k.img" "--persist" "img.nanos"]
         (build {:os "nanos" :kernel "k.img" :persist true :image "img.nanos"}))))

(deftest osv
  (is (= ["osv" "-d" "--cmdline" "foo=bar" "--disk" "d.raw" "--gic" "3" "osv.img"]
         (build {:os "osv" :cmdline "foo=bar" :disk "d.raw" :gic 3 :image "osv.img"}))))

(deftest unikraft
  (is (= ["unikraft" "-d" "--mount" "~/a:/a" "."]
         (build {:os "unikraft" :mounts ["~/a:/a"]})))
  (testing "defaults :path to \".\""
    (is (= ["unikraft" "-d" "."] (build {:os "unikraft"})))))

(deftest string-or-keyword-os
  (is (= ["netbsd" "-d"] (build {:os "netbsd"})))
  (is (= ["netbsd" "-d"] (build {:os :netbsd}))))

(deftest unknown-os-throws
  (is (thrown? clojure.lang.ExceptionInfo (build {:os "plan9"})))
  (try
    (build {:os "plan9"})
    (catch clojure.lang.ExceptionInfo e
      (is (= :unknown-os (:bsdkrun/error (ex-data e)))))))

(deftest no-net-when-net-absent
  (is (= [] (args/net-args nil))))
