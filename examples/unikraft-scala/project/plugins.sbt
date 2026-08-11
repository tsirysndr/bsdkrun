// A unikernel runs one jar with everything in it — including the Scala
// standard library, which `sbt package` alone would leave behind.
addSbtPlugin("com.eed3si9n" % "sbt-assembly" % "2.2.0")
