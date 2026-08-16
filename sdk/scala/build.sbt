// The Scala SDK for bsdkrun.
//
// Deliberately thin on dependencies: upickle for JSON, and nothing else. HTTP
// and WebSocket come from `java.net.http`, which has been in the JDK since 11 —
// the same reason the clojure SDK needs no client library either. That keeps
// the SDK droppable into a codebase already committed to cats-effect, ZIO or
// neither.
ThisBuild / organization := "io.github.tsirysndr"
ThisBuild / version := "0.1.0"

// Scala 3 LTS. The API avoids anything 3-only in its signatures, so a 2.13
// cross-build is a `crossScalaVersions` line away if one is ever wanted.
ThisBuild / scalaVersion := "3.3.4"

lazy val root = (project in file("."))
  .settings(
    name := "bsdkrun",
    description :=
      "Scala SDK for bsdkrun — a Firecracker-style microVM launcher for BSD, " +
        "Linux and unikernel guests.",
    homepage := Some(url("https://github.com/tsirysndr/bsdkrun")),
    licenses := List("MIT" -> url("https://opensource.org/licenses/MIT")),
    libraryDependencies ++= Seq(
      "com.lihaoyi" %% "upickle" % "4.0.2",
      "org.scalameta" %% "munit" % "1.0.4" % Test
    ),
    scalacOptions ++= Seq(
      "-deprecation",
      "-feature",
      "-unchecked",
      // CI treats warnings as errors, matching the elixir SDK's
      // `mix compile --warnings-as-errors`.
      "-Wunused:all",
      "-Xfatal-warnings"
    ),
    Test / fork := true
  )
