// Maven Central is the only registry Scala consumers resolve from by default,
// and it demands more than the other SDKs' registries do: every artifact GPG
// signed, a -sources and -javadoc jar alongside the main one, and a POM
// carrying name/description/url/licenses/scm/developers. These two plugins are
// what supply that.
addSbtPlugin("org.xerial.sbt" % "sbt-sonatype" % "3.12.2")
addSbtPlugin("com.github.sbt" % "sbt-pgp" % "2.3.1")
