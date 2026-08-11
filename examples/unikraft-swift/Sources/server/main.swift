// A Swift HTTP service, to prove it runs as a Unikraft unikernel.
//
// POSIX sockets rather than a server framework: every Swift HTTP package
// pulls in SwiftNIO and a dozen transitive dependencies, and this example is
// about the toolchain reaching the guest, not about the ecosystem.
import Foundation

#if canImport(Glibc)
import Glibc
#endif

let port: UInt16 = 8080

let body = #"{"message": "Hello from Swift on Unikraft!", "swift": "\#(swiftVersion())"}"#

func swiftVersion() -> String {
    #if swift(>=6.0)
    return "6.x"
    #elseif swift(>=5.10)
    return "5.10"
    #else
    return "5.x"
    #endif
}

let response = """
HTTP/1.1 200 OK\r
Content-Type: application/json\r
Content-Length: \(body.utf8.count)\r
Connection: close\r
\r
\(body)
"""

let listener = socket(AF_INET, Int32(SOCK_STREAM.rawValue), 0)
guard listener >= 0 else { fatalError("socket() failed") }

var yes: Int32 = 1
setsockopt(listener, SOL_SOCKET, SO_REUSEADDR, &yes, socklen_t(MemoryLayout<Int32>.size))

var addr = sockaddr_in()
addr.sin_family = sa_family_t(AF_INET)
addr.sin_port = port.bigEndian
addr.sin_addr = in_addr(s_addr: INADDR_ANY)

let bound = withUnsafePointer(to: &addr) {
    $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
        bind(listener, $0, socklen_t(MemoryLayout<sockaddr_in>.size))
    }
}
guard bound == 0 else { fatalError("bind() failed") }
guard listen(listener, 16) == 0 else { fatalError("listen() failed") }

print("Swift \(swiftVersion()) listening on :\(port)")
fflush(stdout)

while true {
    let client = accept(listener, nil, nil)
    if client < 0 { continue }

    var buffer = [UInt8](repeating: 0, count: 1024)
    _ = recv(client, &buffer, buffer.count, 0)

    var out = Array(response.utf8)
    _ = send(client, &out, out.count, 0)
    close(client)
}
