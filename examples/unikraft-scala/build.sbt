ThisBuild / scalaVersion := "3.3.3"

lazy val root = (project in file("."))
  .settings(
    name := "server",
    version := "0.1.0",
    assembly / mainClass := Some("Server"),
    assembly / assemblyJarName := "server-assembly.jar"
  )
