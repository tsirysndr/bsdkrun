%% A minimal, single-request HTTP/1.1 server for `test/client_test.gleam`'s
%% end-to-end HTTP-transport tests — no Hex dependency, per the feature's own
%% suggested testing approach ("stand up a minimal local HTTP server via
%% :gen_tcp FFI"). It is not part of the SDK; it lives under test/ and is
%% only ever a build dependency of the test suite.
%%
%% `start/2` listens on a free loopback port, replies to exactly one HTTP
%% request with the given status/body, and returns the port so a test can
%% point `bsdkrun/client` at `http://127.0.0.1:<port>/graphql`.
-module(fake_http_server_ffi).

-export([start/2, putenv/2, unsetenv/1]).

%% `client.from_env`'s tests need to set/clear real OS environment variables
%% around each case — `os:putenv/2`/`os:unsetenv/1` want charlists, not the
%% UTF-8 binaries a Gleam `String` is, hence these tiny wrappers.
putenv(Name, Value) ->
    true = os:putenv(binary_to_list(Name), binary_to_list(Value)),
    nil.

unsetenv(Name) ->
    true = os:unsetenv(binary_to_list(Name)),
    nil.

start(Status, Body) ->
    {ok, Listen} = gen_tcp:listen(0, [binary, {active, false}, {reuseaddr, true}]),
    {ok, Port} = inet:port(Listen),
    Self = erlang:self(),
    spawn(fun() -> accept_one(Listen, Status, Body, Self) end),
    {ok, Port}.

accept_one(Listen, Status, Body, _Owner) ->
    {ok, Socket} = gen_tcp:accept(Listen, 30000),
    %% We don't need the request beyond having received *a* request — read
    %% whatever is immediately available (the daemon's real handler would
    %% parse it; this fake only cares that a POST arrived) and reply.
    _ = gen_tcp:recv(Socket, 0, 5000),
    Reason = reason_phrase(Status),
    Response = iolist_to_binary([
        "HTTP/1.1 ", integer_to_list(Status), " ", Reason, "\r\n",
        "content-type: application/json\r\n",
        "content-length: ", integer_to_list(byte_size(Body)), "\r\n",
        "connection: close\r\n",
        "\r\n",
        Body
    ]),
    ok = gen_tcp:send(Socket, Response),
    gen_tcp:close(Socket),
    gen_tcp:close(Listen).

reason_phrase(200) -> "OK";
reason_phrase(401) -> "Unauthorized";
reason_phrase(500) -> "Internal Server Error";
reason_phrase(_) -> "OK".
