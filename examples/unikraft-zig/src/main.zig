//! A Zig HTTP service, to prove it runs as a Unikraft unikernel.
//!
//! The response is written by hand rather than through std.http: that API
//! has changed shape in most recent Zig releases, and a raw socket keeps
//! this example building across more of them.
const std = @import("std");
const builtin = @import("builtin");

const port = 8080;

const body = "{\"message\": \"Hello from Zig on Unikraft!\", \"zig\": \"" ++
    builtin.zig_version_string ++ "\"}";

// Built at comptime so Content-Length cannot drift from the body it
// describes — a mismatch leaves the client waiting for bytes that never
// come.
const response = std.fmt.comptimePrint(
    "HTTP/1.1 200 OK\r\n" ++
        "Content-Type: application/json\r\n" ++
        "Content-Length: {d}\r\n" ++
        "Connection: close\r\n" ++
        "\r\n{s}",
    .{ body.len, body },
);

pub fn main() !void {
    const address = try std.net.Address.parseIp("0.0.0.0", port);
    var server = try address.listen(.{ .reuse_address = true });
    defer server.deinit();

    std.debug.print("Zig {s} listening on :{d}\n", .{ builtin.zig_version_string, port });

    while (true) {
        const conn = server.accept() catch continue;
        defer conn.stream.close();

        var buf: [1024]u8 = undefined;
        _ = conn.stream.read(&buf) catch {};
        conn.stream.writeAll(response) catch {};
    }
}
