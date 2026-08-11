open Lwt.Infix

let port = 8080
let body = "Hello from MirageOS on bsdkrun\n"

let response =
  Printf.sprintf
    "HTTP/1.1 200 OK\r\n\
     Content-Type: text/plain\r\n\
     Content-Length: %d\r\n\
     Connection: close\r\n\
     \r\n\
     %s"
    (String.length body) body

module Main (S : Tcpip.Stack.V4V6) = struct
  (* Read the request before answering. Writing and closing immediately would
     work often enough to look right, but a close with unread data in the
     receive queue sends an RST, and curl reports that as a reset connection
     rather than as the response it already had. *)
  let serve flow =
    S.TCP.read flow >>= fun _request ->
    S.TCP.write flow (Cstruct.of_string response) >>= function
    | Ok () -> S.TCP.close flow
    | Error e ->
        Logs.warn (fun f -> f "write failed: %a" S.TCP.pp_write_error e);
        S.TCP.close flow

  let start stack =
    S.TCP.listen (S.tcp stack) ~port serve;
    Logs.info (fun f -> f "listening on port %d" port);
    S.listen stack
end
