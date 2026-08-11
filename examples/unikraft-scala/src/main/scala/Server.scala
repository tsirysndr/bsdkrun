import com.sun.net.httpserver.{HttpExchange, HttpServer}
import java.net.InetSocketAddress
import java.nio.charset.StandardCharsets

/** A Scala HTTP service, to prove it runs as a Unikraft unikernel. */
object Server:
  private val Port = 8080

  def main(args: Array[String]): Unit =
    val server = HttpServer.create(InetSocketAddress("0.0.0.0", Port), 0)

    server.createContext("/", (exchange: HttpExchange) =>
      val body =
        s"""{"message": "Hello from Scala on Unikraft!", "scala": "${util.Properties.versionNumberString}", "path": "${exchange.getRequestURI.getPath}"}"""
      val bytes = body.getBytes(StandardCharsets.UTF_8)

      exchange.getResponseHeaders.set("Content-Type", "application/json")
      exchange.sendResponseHeaders(200, bytes.length)
      val out = exchange.getResponseBody
      try out.write(bytes) finally out.close()
    )

    // The guest is one CPU; a thread pool would only add threads for it to
    // context-switch between.
    server.setExecutor(null)
    println(s"Scala ${util.Properties.versionNumberString} listening on :$Port")
    Console.flush()
    server.start()
