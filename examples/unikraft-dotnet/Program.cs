// A C# HTTP service, to prove .NET runs as a Unikraft unikernel.
//
// A TcpListener rather than ASP.NET: Kestrel would bring the whole web stack
// into a rootfs that has to be resident twice at boot, and this example is
// about the runtime reaching the guest.
using System.Net;
using System.Net.Sockets;
using System.Text;

const int port = 8080;

var listener = new TcpListener(IPAddress.Any, port);
listener.Start();

Console.WriteLine($".NET {Environment.Version} listening on :{port}");
Console.Out.Flush();

while (true)
{
    using var client = listener.AcceptTcpClient();
    using var stream = client.GetStream();

    var request = new byte[1024];
    try { stream.Read(request, 0, request.Length); } catch { }

    var body = $"{{\"message\": \"Hello from C# on Unikraft!\", \"dotnet\": \"{Environment.Version}\"}}";
    var response = "HTTP/1.1 200 OK\r\n"
                 + "Content-Type: application/json\r\n"
                 + $"Content-Length: {Encoding.UTF8.GetByteCount(body)}\r\n"
                 + "Connection: close\r\n"
                 + "\r\n"
                 + body;

    var bytes = Encoding.UTF8.GetBytes(response);
    stream.Write(bytes, 0, bytes.Length);
}
