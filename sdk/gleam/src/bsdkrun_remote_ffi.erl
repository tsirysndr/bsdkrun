%% Erlang FFI for the bsdkrun Gleam SDK's remote-client feature
%% (`bsdkrun/client`, `bsdkrun/graphql_transport`, `bsdkrun/ws`,
%% `bsdkrun/subject`).
%%
%% Kept separate from `bsdkrun_ffi.erl` (which backs the *local*,
%% CLI-shelling half of this SDK) because this half talks to a network
%% daemon instead of a subprocess: HTTP via `:httpc`/`:inets`, a hand-rolled
%% `graphql-transport-ws` client over `:gen_tcp`/`:ssl`, and `:crypto` for
%% the WebSocket handshake. No Hex package provides any of this — `:httpc`,
%% `:gen_tcp`, `:ssl` and `:crypto` all ship as part of the Erlang/OTP
%% standard distribution (the `inets`, `ssl` and `crypto` OTP applications),
%% not as separate dependencies, so using them here does not add anything to
%% `gleam.toml`.
-module(bsdkrun_remote_ffi).

%% `catch Expr` (rather than `try Expr catch _:_ -> ok end`) is used
%% throughout the connection process below to make socket teardown/best-
%% effort sends genuinely one-liners; OTP considers the bare form
%% deprecated in favour of `try`, but it still works and reads far better
%% at every one of these call sites (all "attempt this, ignore any error").
-compile([nowarn_deprecated_catch]).

-export([
    %% subject.gleam — a minimal, hand-rolled process.Subject
    self/0,
    new_tag/0,
    raw_send/3,
    raw_receive/2,
    %% ws.gleam — pure protocol helpers needing :crypto
    sha1/1,
    strong_rand_bytes/1,
    %% graphql_transport.gleam — HTTP POST via :httpc
    http_post/3,
    %% ws.gleam — the WebSocket connection process
    ws_start/8,
    ws_subscribe/6,
    ws_unsubscribe/2,
    ws_close/1,
    ws_alive/1,
    ws_cache_get/1,
    ws_cache_put/2,
    %% client.gleam — the request()/subscribe() escape hatch
    dynamic_to_json/1
]).

%% ============================================================================
%% subject.gleam
%% ============================================================================

self() -> erlang:self().

%% A tag unique enough that two `Subject`s never collide inside one mailbox.
new_tag() ->
    integer_to_binary(erlang:unique_integer([positive, monotonic])).

raw_send(Pid, Tag, Message) ->
    Pid ! {subject_msg, Tag, Message},
    nil.

%% A negative timeout means "wait forever" (Erlang `after infinity`) — used
%% by the background forwarder loops in `bsdkrun/client`, which have nothing
%% else to do and are always eventually woken by a terminal event even on a
%% dead connection (the WS actor notifies every open subscription when its
%% socket closes, for any reason).
raw_receive(Tag, TimeoutMs) ->
    Timeout = case TimeoutMs < 0 of
        true -> infinity;
        false -> TimeoutMs
    end,
    receive
        {subject_msg, Tag, Message} -> {ok, Message}
    after Timeout ->
        {error, nil}
    end.

%% ============================================================================
%% ws.gleam — pure helpers
%% ============================================================================

sha1(Data) -> crypto:hash(sha, Data).

strong_rand_bytes(N) -> crypto:strong_rand_bytes(N).

%% ============================================================================
%% graphql_transport.gleam
%% ============================================================================

%% POST `Body` (already-serialized JSON) to `Url` with the bearer token this
%% SDK always sends. Returns `{ok, {StatusCode, ResponseBodyBinary}}` on
%% anything that got a full HTTP response back — including a 401 or a 5xx,
%% which `bsdkrun/graphql_transport` interprets, not this function — or
%% `{error, ReasonBinary}` only for a genuine transport failure (refused,
%% timed out, DNS, TLS handshake failed, ...).
http_post(Url, Token, Body) ->
    ok = ensure_started(inets),
    ok = ensure_started(ssl),
    UrlStr = binary_to_list(Url),
    Headers = [
        {"authorization", "Bearer " ++ binary_to_list(Token)},
        {"accept", "application/json"}
    ],
    ContentType = "application/json",
    HttpOpts = [{timeout, 30000}, {connect_timeout, 10000} | tls_opts(UrlStr)],
    Request = {UrlStr, Headers, ContentType, Body},
    try httpc:request(post, Request, HttpOpts, [{body_format, binary}]) of
        {ok, {{_HttpVersion, Status, _ReasonPhrase}, _RespHeaders, RespBody}} ->
            {ok, {Status, RespBody}};
        {error, Reason} ->
            {error, reason_to_binary(Reason)}
    catch
        Class:Reason2 ->
            {error, reason_to_binary({Class, Reason2})}
    end.

%% A loopback daemon's TLS cert is very often self-signed (see
%% `daemon/README.md`'s Quick start), but this defaults to full system-CA
%% verification anyway — same policy as `daemon/src/client.rs`'s own remote
%% client (`ClientTlsConfig::new().with_native_roots()`), and what every
%% other language's HTTP client (`fetch`, `httpx`, `net/http`, ...) does by
%% default. A self-signed daemon therefore needs a real certificate (or a
%% reverse proxy that terminates TLS with one) to be reached over `https://`
%% from this SDK, exactly as it would from a browser.
tls_opts(UrlStr) ->
    case string:prefix(UrlStr, "https://") of
        nomatch ->
            [];
        _ ->
            [{ssl, [{verify, verify_peer}, {cacerts, public_key:cacerts_get()}]}]
    end.

ensure_started(App) ->
    case application:ensure_all_started(App) of
        {ok, _} -> ok;
        {error, _} -> ok
    end.

reason_to_binary(Reason) ->
    unicode:characters_to_binary(io_lib:format("~p", [Reason])).

%% ============================================================================
%% ws.gleam — the connection process
%% ============================================================================
%%
%% One process per socket. Owns the socket, a leftover-bytes buffer (a
%% frame's header or payload can straddle two TCP reads), whether
%% `connection_ack` has arrived yet, subscribe frames queued while it hasn't,
%% and the subscription-id -> reply-{Pid,Tag} map. All protocol *decisions*
%% (frame encode/decode, JSON envelope build/parse, the handshake accept-key)
%% are delegated to the pure functions in `bsdkrun@ws` (the compiled form of
%% `bsdkrun/ws.gleam` — Gleam compiles to a plain Erlang module, so calling
%% it by its qualified name from here is an ordinary same-VM call, not FFI in
%% the other direction). This process only does I/O and holds state.

%% Spawn a connection process: connect, perform the RFC 6455 handshake, and
%% report the outcome to `{ReplyPid, ReplyTag}` via `raw_send/3` — either
%% `{connected, ActorPid}` or `{connect_failed, ReasonBinary}`. On success the
%% process keeps running, having already sent `connection_init`.
ws_start(Host, Port, Path, Tls, Token, ClientKey, ReplyPid, ReplyTag) ->
    spawn(fun() ->
        ws_actor_init(Host, Port, Path, Tls, Token, ClientKey, ReplyPid, ReplyTag)
    end),
    nil.

ws_actor_init(Host, Port, Path, Tls, Token, ClientKey, ReplyPid, ReplyTag) ->
    case ws_connect_socket(Host, Port, Tls) of
        {ok, Socket} ->
            case ws_handshake(Socket, Tls, Host, Port, Path, ClientKey) of
                {ok, Trailing} ->
                    raw_send(ReplyPid, ReplyTag, {connected, erlang:self()}),
                    ws_set_active(Socket, Tls),
                    InitFrame = 'bsdkrun@ws':encode_frame(
                        {text_frame, 'bsdkrun@ws':build_connection_init(Token)}
                    ),
                    _ = ws_raw_send(Socket, Tls, InitFrame),
                    ws_loop(#{
                        socket => Socket,
                        tls => Tls,
                        buffer => Trailing,
                        acked => false,
                        pending => [],
                        subs => #{}
                    });
                {error, Reason} ->
                    _ = ws_close_socket(Socket, Tls),
                    raw_send(ReplyPid, ReplyTag, {connect_failed, reason_to_binary(Reason)})
            end;
        {error, Reason} ->
            raw_send(ReplyPid, ReplyTag, {connect_failed, reason_to_binary(Reason)})
    end.

ws_connect_socket(Host, Port, true) ->
    HostStr = binary_to_list(Host),
    ok = ensure_started(ssl),
    ssl:connect(HostStr, Port, [
        binary,
        {active, false},
        {packet, 0},
        {verify, verify_peer},
        {cacerts, public_key:cacerts_get()},
        {server_name_indication, HostStr}
    ], 10000);
ws_connect_socket(Host, Port, false) ->
    gen_tcp:connect(binary_to_list(Host), Port, [binary, {active, false}, {packet, 0}], 10000).

ws_close_socket(Socket, true) -> catch ssl:close(Socket);
ws_close_socket(Socket, false) -> catch gen_tcp:close(Socket).

ws_raw_send(Socket, true, Data) -> ssl:send(Socket, Data);
ws_raw_send(Socket, false, Data) -> gen_tcp:send(Socket, Data).

ws_set_active(Socket, true) -> ssl:setopts(Socket, [{active, true}]);
ws_set_active(Socket, false) -> inet:setopts(Socket, [{active, true}]).

%% -- the HTTP/1.1 Upgrade handshake ------------------------------------------

ws_handshake(Socket, Tls, Host, Port, Path, ClientKey) ->
    Req = handshake_request(Host, Port, Path, ClientKey),
    case ws_raw_send(Socket, Tls, Req) of
        ok ->
            case read_http_response(Socket, Tls, <<>>, 10000) of
                {ok, StatusLine, Headers, Trailing} ->
                    verify_handshake(StatusLine, Headers, ClientKey, Trailing);
                {error, Reason} ->
                    {error, Reason}
            end;
        {error, Reason} ->
            {error, Reason}
    end.

handshake_request(Host, Port, Path, ClientKey) ->
    HostHeader = case Port of
        80 -> Host;
        443 -> Host;
        _ -> <<Host/binary, ":", (integer_to_binary(Port))/binary>>
    end,
    iolist_to_binary([
        "GET ", Path, " HTTP/1.1\r\n",
        "Host: ", HostHeader, "\r\n",
        "Upgrade: websocket\r\n",
        "Connection: Upgrade\r\n",
        "Sec-WebSocket-Key: ", ClientKey, "\r\n",
        "Sec-WebSocket-Version: 13\r\n",
        "Sec-WebSocket-Protocol: graphql-transport-ws\r\n",
        "\r\n"
    ]).

%% Read off the socket until the blank line ending the HTTP response headers
%% is seen, then split it into a status line, a header list, and whatever
%% bytes (if any) arrived after the headers in the same read — which, since a
%% server is free to pipeline the first WS frame right behind the 101
%% response, become the connection's initial frame buffer rather than being
%% dropped.
read_http_response(Socket, Tls, Acc, TimeoutMs) ->
    case binary:match(Acc, <<"\r\n\r\n">>) of
        {Pos, _Len} ->
            HeaderPart = binary:part(Acc, 0, Pos),
            Trailing = binary:part(Acc, Pos + 4, byte_size(Acc) - Pos - 4),
            [StatusLine | HeaderLines] = binary:split(HeaderPart, <<"\r\n">>, [global]),
            {ok, StatusLine, parse_headers(HeaderLines), Trailing};
        nomatch ->
            case ws_recv(Socket, Tls, TimeoutMs) of
                {ok, Data} -> read_http_response(Socket, Tls, <<Acc/binary, Data/binary>>, TimeoutMs);
                {error, Reason} -> {error, Reason}
            end
    end.

ws_recv(Socket, true, TimeoutMs) -> ssl:recv(Socket, 0, TimeoutMs);
ws_recv(Socket, false, TimeoutMs) -> gen_tcp:recv(Socket, 0, TimeoutMs).

parse_headers(Lines) ->
    lists:filtermap(fun(Line) ->
        case binary:split(Line, <<": ">>) of
            [Name, Value] -> {true, {string:lowercase(binary_to_list(Name)), Value}};
            _ -> false
        end
    end, Lines).

verify_handshake(StatusLine, Headers, ClientKey, Trailing) ->
    case status_code(StatusLine) of
        101 ->
            case lists:keyfind("sec-websocket-accept", 1, Headers) of
                {_, AcceptValue} ->
                    Expected = 'bsdkrun@ws':compute_accept_key(ClientKey),
                    Got = string:trim(binary_to_list(AcceptValue)),
                    case Got =:= binary_to_list(unicode:characters_to_binary(Expected)) of
                        true -> {ok, Trailing};
                        false -> {error, "server sent an invalid Sec-WebSocket-Accept"}
                    end;
                false ->
                    {error, "server did not send Sec-WebSocket-Accept"}
            end;
        Code ->
            {error, io_lib:format("handshake failed with HTTP ~p", [Code])}
    end.

status_code(StatusLine) ->
    case binary:split(StatusLine, <<" ">>, [global]) of
        [_Http, CodeBin | _] ->
            case catch binary_to_integer(CodeBin) of
                Code when is_integer(Code) -> Code;
                _ -> 0
            end;
        _ -> 0
    end.

%% -- the main loop ------------------------------------------------------------

ws_loop(State) ->
    #{socket := Socket, tls := Tls} = State,
    receive
        {tcp, Socket, Data} ->
            ws_loop(handle_data(State, Data));
        {ssl, Socket, Data} ->
            ws_loop(handle_data(State, Data));
        {tcp_closed, Socket} ->
            handle_closed(State);
        {ssl_closed, Socket} ->
            handle_closed(State);
        {tcp_error, Socket, _Reason} ->
            handle_closed(State);
        {ssl_error, Socket, _Reason} ->
            handle_closed(State);
        {control, {subscribe, Id, Query, VariablesJson, ReplyPid, ReplyTag}} ->
            Subs = maps:put(Id, {ReplyPid, ReplyTag}, maps:get(subs, State)),
            FrameText = 'bsdkrun@ws':build_subscribe(Id, Query, VariablesJson),
            FrameBytes = 'bsdkrun@ws':encode_frame({text_frame, FrameText}),
            State1 = State#{subs := Subs},
            case maps:get(acked, State1) of
                true ->
                    _ = ws_raw_send(Socket, Tls, FrameBytes),
                    ws_loop(State1);
                false ->
                    Pending = maps:get(pending, State1) ++ [FrameBytes],
                    ws_loop(State1#{pending := Pending})
            end;
        {control, {unsubscribe, Id}} ->
            Subs = maps:remove(Id, maps:get(subs, State)),
            FrameText = 'bsdkrun@ws':build_complete(Id),
            FrameBytes = 'bsdkrun@ws':encode_frame({text_frame, FrameText}),
            catch ws_raw_send(Socket, Tls, FrameBytes),
            case maps:size(Subs) of
                0 -> catch ws_close_socket(Socket, Tls);
                _ -> ws_loop(State#{subs := Subs})
            end;
        {control, close} ->
            catch ws_close_socket(Socket, Tls),
            ok;
        _Other ->
            ws_loop(State)
    end.

handle_data(State, Data) ->
    #{buffer := Buffer} = State,
    drain(State#{buffer := <<Buffer/binary, Data/binary>>}).

drain(State) ->
    #{buffer := Buffer} = State,
    case 'bsdkrun@ws':decode_frame(Buffer) of
        {decoded, Frame, Rest} ->
            drain(handle_frame(State#{buffer := Rest}, Frame));
        incomplete ->
            State;
        {frame_error, _Reason} ->
            %% A malformed frame from the server. The connection is no longer
            %% trustworthy; stop reading and let the next socket event (the
            %% server closing right after sending garbage, in practice) tear
            %% it down via handle_closed/1. Simplification: does not itself
            %% send a Close frame back.
            State
    end.

handle_frame(State, {text_frame, Text}) ->
    handle_frame_text(State, Text);
handle_frame(State, {ping_frame, Payload}) ->
    #{socket := Socket, tls := Tls} = State,
    Pong = 'bsdkrun@ws':encode_frame({pong_frame, Payload}),
    catch ws_raw_send(Socket, Tls, Pong),
    State;
handle_frame(State, _OtherFrame) ->
    %% binary/pong frames: nothing in this protocol sends them, ignored.
    %% close_frame: a spec-compliant server closes the TCP connection right
    %% after, which arrives as {tcp_closed,_}/{ssl_closed,_} and is handled
    %% there — not specially reacted to here (documented shortcut).
    State.

dispatch_incoming(State, ack) ->
    #{socket := Socket, tls := Tls, pending := Pending} = State,
    lists:foreach(fun(Bytes) -> catch ws_raw_send(Socket, Tls, Bytes) end, Pending),
    State#{acked := true, pending := []};
dispatch_incoming(State, {next, Id, Data}) ->
    notify(State, Id, {raw_next, Data}),
    State;
dispatch_incoming(State, {error_msg, Id, Message}) ->
    notify(State, Id, {raw_error, Message}),
    State#{subs := maps:remove(Id, maps:get(subs, State))};
dispatch_incoming(State, {complete, Id}) ->
    notify(State, Id, raw_complete),
    State#{subs := maps:remove(Id, maps:get(subs, State))};
dispatch_incoming(State, ping) ->
    #{socket := Socket, tls := Tls} = State,
    Pong = 'bsdkrun@ws':encode_frame({text_frame, 'bsdkrun@ws':build_pong()}),
    catch ws_raw_send(Socket, Tls, Pong),
    State.

notify(State, Id, Message) ->
    case maps:find(Id, maps:get(subs, State)) of
        {ok, {Pid, Tag}} -> raw_send(Pid, Tag, Message);
        error -> ok
    end,
    ok.

handle_closed(State) ->
    #{acked := Acked, subs := Subs} = State,
    Event = case Acked of
        true -> {raw_error, <<"the connection to the daemon was closed">>};
        false -> {raw_auth_error, <<"the daemon rejected this token">>}
    end,
    maps:foreach(fun(_Id, {Pid, Tag}) -> raw_send(Pid, Tag, Event) end, Subs),
    ok.

%% Feed a decoded `{text_frame, Text}` (produced by `handle_frame`'s catch-all
%% not matching it — dispatched here instead, from `drain/1`'s caller via
%% `handle_frame/2`'s clause below) through `bsdkrun@ws:parse_incoming/1` and
%% on to `dispatch_incoming/2`.
handle_frame_text(State, Text) ->
    case 'bsdkrun@ws':parse_incoming(Text) of
        {ok, Incoming} -> dispatch_incoming(State, Incoming);
        {error, nil} -> State
    end.

%% ============================================================================
%% ws.gleam — client-facing control operations
%% ============================================================================

ws_subscribe(Pid, Id, Query, VariablesJson, ReplyPid, ReplyTag) ->
    Pid ! {control, {subscribe, Id, Query, VariablesJson, ReplyPid, ReplyTag}},
    nil.

ws_unsubscribe(Pid, Id) ->
    Pid ! {control, {unsubscribe, Id}},
    nil.

ws_close(Pid) ->
    Pid ! {control, close},
    nil.

ws_alive(Pid) ->
    erlang:is_process_alive(Pid).

%% -- the shared-socket cache --------------------------------------------------
%%
%% `bsdkrun/client`'s `Client` is an immutable url/token pair with nowhere of
%% its own to hold a live connection pid, so "one shared socket per client
%% connection" is implemented as a small node-global cache keyed by
%% `url <> token`, in `persistent_term` — the same mechanism
%% `bsdkrun_ffi.erl`'s binary-path override already uses. Reads are cheap
%% (persistent_term is built for read-heavy, write-rarely data, which this
%% is: one write per new connection, unbounded reads for every subsequent
%% call against the same `Client`).

-define(WS_CACHE_KEY(Key), {bsdkrun_ws_ffi, Key}).

ws_cache_get(Key) ->
    case persistent_term:get(?WS_CACHE_KEY(Key), undefined) of
        undefined ->
            {error, nil};
        Pid ->
            case erlang:is_process_alive(Pid) of
                true -> {ok, Pid};
                false ->
                    _ = persistent_term:erase(?WS_CACHE_KEY(Key)),
                    {error, nil}
            end
    end.

ws_cache_put(Key, Pid) ->
    persistent_term:put(?WS_CACHE_KEY(Key), Pid),
    nil.

%% ============================================================================
%% client.gleam — the request()/subscribe() escape hatch
%% ============================================================================
%%
%% `client.request`/`client.subscribe` take `variables: Dynamic` rather than
%% a `gleam_json`-built `Json`, since they are meant for an operation the
%% typed API does not cover and the caller may not want to hand-build a
%% `Json` value for. A `Dynamic` on the Erlang target is the underlying term
%% itself, so this is a small recursive term -> JSON-text encoder covering
%% the shapes a caller is expected to actually pass: maps (objects — binary,
%% atom or integer keys), lists (arrays), binaries (strings), integers,
%% floats, `true`/`false`, and Gleam's `Option` (`{some, X}` -> encode `X`,
%% the atom `none` -> `null`, alongside the atoms `nil`/`undefined` for
%% plain Erlang callers). Anything else falls back to its `~p`-formatted
%% representation as a JSON string rather than crashing.
dynamic_to_json(Term) ->
    iolist_to_binary(encode_term(Term)).

encode_term(Term) when is_map(Term) ->
    Entries = maps:fold(fun(K, V, Acc) ->
        [[encode_string(key_to_binary(K)), $:, encode_term(V)] | Acc]
    end, [], Term),
    ["{", lists:join($,, Entries), "}"];
encode_term(Term) when is_list(Term) ->
    ["[", lists:join($,, [encode_term(X) || X <- Term]), "]"];
encode_term(Term) when is_binary(Term) ->
    encode_string(Term);
encode_term(Term) when is_integer(Term) ->
    integer_to_list(Term);
encode_term(Term) when is_float(Term) ->
    float_to_list(Term, [short]);
encode_term(true) -> "true";
encode_term(false) -> "false";
encode_term(nil) -> "null";
encode_term(undefined) -> "null";
encode_term(none) -> "null";
encode_term({some, V}) -> encode_term(V);
encode_term(Term) when is_atom(Term) ->
    encode_string(atom_to_binary(Term, utf8));
encode_term(Term) ->
    encode_string(unicode:characters_to_binary(io_lib:format("~p", [Term]))).

key_to_binary(K) when is_binary(K) -> K;
key_to_binary(K) when is_atom(K) -> atom_to_binary(K, utf8);
key_to_binary(K) when is_integer(K) -> integer_to_binary(K).

encode_string(Bin) ->
    [$", escape_json_string(Bin), $"].

escape_json_string(Bin) -> escape_json_string(Bin, []).

escape_json_string(<<>>, Acc) ->
    lists:reverse(Acc);
escape_json_string(<<$", Rest/binary>>, Acc) ->
    escape_json_string(Rest, [$", $\\ | Acc]);
escape_json_string(<<$\\, Rest/binary>>, Acc) ->
    escape_json_string(Rest, [$\\, $\\ | Acc]);
escape_json_string(<<$\n, Rest/binary>>, Acc) ->
    escape_json_string(Rest, [$n, $\\ | Acc]);
escape_json_string(<<$\r, Rest/binary>>, Acc) ->
    escape_json_string(Rest, [$r, $\\ | Acc]);
escape_json_string(<<$\t, Rest/binary>>, Acc) ->
    escape_json_string(Rest, [$t, $\\ | Acc]);
escape_json_string(<<C/utf8, Rest/binary>>, Acc) when C < 16#20 ->
    Hex = lists:flatten(io_lib:format("\\u~4.16.0B", [C])),
    escape_json_string(Rest, lists:reverse(Hex) ++ Acc);
escape_json_string(<<C/utf8, Rest/binary>>, Acc) ->
    escape_json_string(Rest, [<<C/utf8>> | Acc]);
escape_json_string(<<_, Rest/binary>>, Acc) ->
    escape_json_string(Rest, Acc).
