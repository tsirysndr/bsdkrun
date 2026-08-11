package dev.bsdkrun;

import com.sun.net.httpserver.HttpServer;
import java.io.IOException;
import java.io.OutputStream;
import java.net.InetSocketAddress;
import java.nio.charset.StandardCharsets;

/** A Java HTTP service, to prove the JVM runs as a Unikraft unikernel. */
public final class Server {
    private static final int PORT = 8080;

    public static void main(String[] args) throws IOException {
        HttpServer server = HttpServer.create(new InetSocketAddress("0.0.0.0", PORT), 0);

        server.createContext("/", exchange -> {
            String body = String.format(
                    "{\"message\": \"Hello from Java on Unikraft!\", \"java\": \"%s\", \"path\": \"%s\"}",
                    System.getProperty("java.version"), exchange.getRequestURI().getPath());
            byte[] bytes = body.getBytes(StandardCharsets.UTF_8);

            exchange.getResponseHeaders().set("Content-Type", "application/json");
            exchange.sendResponseHeaders(200, bytes.length);
            try (OutputStream out = exchange.getResponseBody()) {
                out.write(bytes);
            }
        });

        // The guest is one CPU; a thread pool here would only add threads
        // for it to context-switch between.
        server.setExecutor(null);
        System.out.println("Java " + System.getProperty("java.version") + " listening on :" + PORT);
        System.out.flush();
        server.start();
    }
}
