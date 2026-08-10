(ns user
  "Auto-loaded REPL helpers. Drops every console namespace into scope under
  short aliases so you can poke around immediately.

      user=> (help)
      user=> (build/release)
      user=> (sdk/test :clojure)     ;; lang is always a keyword (atom)
      user=> (web/dev)"
  (:require [console.core    :as c]
            [console.shell   :as sh]
            [console.build   :as build]
            [console.sdk     :as sdk]
            [console.web     :as web]
            [console.desktop :as desktop]))

(def help c/help)
(def ls   c/ls)

(println)
(println "bsdkrun Console — REPL loaded. Try (help) or (ls).")
(println "Aliases in scope: c, sh, build, sdk, web, desktop")
(println)
