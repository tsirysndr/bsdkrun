%% Erlang FFI for the bsdkrun Gleam SDK.
%%
%% Everything Gleam cannot express on its own: spawning the `bsdkrun` binary
%% and capturing its streams, PATH/env lookups, and the cached binary override.
-module(bsdkrun_ffi).

-export([
    run/6,
    run_bits/4,
    run_inherit/2,
    find_executable/1,
    get_env/1,
    file_exists/1,
    cwd/0,
    set_override/1,
    get_override/0,
    clear_override/0
]).

-define(OVERRIDE_KEY, {bsdkrun_ffi, binary_override}).

%% --- running the binary -----------------------------------------------------

%% Run `Bin Args` to completion with stdout and stderr captured separately.
%%
%% An Erlang port gives us one merged output stream, so — like the Elixir SDK —
%% the child is wrapped in a tiny `/bin/sh` that redirects stderr (and
%% optionally stdin) through temp files we read back and delete. Paths travel
%% via the environment so no shell quoting of user data is ever required.
%%
%% Returns the Gleam `cli.Output` record — `{output, Stdout, Stderr, ExitCode}`.
run(Bin, Args, Env, Stdin, OnStdout, OnStderr) ->
    StderrPath = tmp_path("stderr"),
    %% `Stdin` is a Gleam `Option(String)`: `none` or `{some, Binary}`.
    {Redirects, StdinEnv, Cleanup} =
        case Stdin of
            none ->
                {"2>\"$__BSDKRUN_STDERR\"", [], []};
            {some, Data} ->
                StdinPath = tmp_path("stdin"),
                ok = file:write_file(StdinPath, Data),
                {"0<\"$__BSDKRUN_STDIN\" 2>\"$__BSDKRUN_STDERR\"",
                 [{"__BSDKRUN_STDIN", StdinPath}],
                 [StdinPath]}
        end,
    Script = "exec \"$@\" " ++ Redirects,
    PortEnv = [{"__BSDKRUN_STDERR", StderrPath} | StdinEnv] ++ env_pairs(Env),
    try
        {Stdout, Code} =
            port_run_stream("/bin/sh", ["-c", Script, "sh", Bin | Args], PortEnv,
                            OnStdout, OnStderr, StderrPath),
        {output, Stdout, read_file_or_empty(StderrPath), Code}
    after
        _ = file:delete(StderrPath),
        [file:delete(P) || P <- Cleanup]
    end.

%% Same transfer, but typed for arbitrary bytes: stdin is an `Option(BitArray)`
%% and stdout comes back as a `BitArray` rather than a Gleam `String`.
%%
%% The bytes were never touched either way — `run/6` writes stdin with
%% `file:write_file` and reads stdout straight off the port — so this only
%% changes the Gleam-side type. It has to exist because a Gleam `String` must
%% be valid UTF-8, and `cp ID:path -` reading a PNG is not.
%%
%% Returns the Gleam `cli.BinaryOutput` record.
run_bits(Bin, Args, Env, Stdin) ->
    {output, Stdout, Stderr, Code} = run(Bin, Args, Env, Stdin, none, none),
    {binary_output, Stdout, Stderr, Code}.

%% Run `Bin Args` with the streams wired straight to the node's own stdio, for
%% interactive subcommands like `bsdkrun shell`. Returns the exit code.
%%
%% Erlang ports cannot hand a child the real controlling TTY, so this shells
%% out through the SSL-free `open_port` path only to wait on it; the child
%% inherits the beam's file descriptors via `sh -c ... <&0 >&1 2>&2`.
run_inherit(Bin, Args) ->
    Script = "exec \"$@\" <&0 >&1 2>&2",
    {_Out, Code} = port_run("/bin/sh", ["-c", Script, "sh", Bin | Args], []),
    Code.

port_run(Executable, Args, Env) ->
    Opts = [
        {args, Args},
        {env, Env},
        binary,
        exit_status,
        use_stdio,
        hide
    ],
    Port = erlang:open_port({spawn_executable, Executable}, Opts),
    collect(Port, []).

port_run_stream(Executable, Args, Env, OnStdout, OnStderr, StderrPath) ->
    Opts = [{args, Args}, {env, Env}, binary, exit_status, use_stdio, hide],
    Port = erlang:open_port({spawn_executable, Executable}, Opts),
    collect_stream(Port, [], OnStdout, OnStderr, StderrPath, 0).

collect_stream(Port, Acc, OnStdout, OnStderr, Path, Offset) ->
    NewOffset = emit_file_delta(Path, Offset, OnStderr),
    receive
        {Port, {data, Data}} ->
            emit(OnStdout, Data),
            collect_stream(Port, [Data | Acc], OnStdout, OnStderr, Path, NewOffset);
        {Port, {exit_status, Code}} ->
            _ = emit_file_delta(Path, NewOffset, OnStderr),
            {iolist_to_binary(lists:reverse(Acc)), Code};
        {'EXIT', Port, _Reason} -> {iolist_to_binary(lists:reverse(Acc)), 1}
    after 10 ->
        collect_stream(Port, Acc, OnStdout, OnStderr, Path, NewOffset)
    end.

emit(none, _Data) -> ok;
emit({some, Fun}, Data) -> Fun(Data).

emit_file_delta(Path, Offset, Callback) ->
    case file:read_file(Path) of
        {ok, Contents} when byte_size(Contents) > Offset ->
            Size = byte_size(Contents),
            emit(Callback, binary:part(Contents, Offset, Size - Offset)),
            Size;
        _ -> Offset
    end.

%% Messages from a single port arrive in order, so every `data` chunk emitted
%% before the child exited is already queued ahead of `exit_status`.
collect(Port, Acc) ->
    receive
        {Port, {data, Data}} ->
            collect(Port, [Data | Acc]);
        {Port, {exit_status, Code}} ->
            {iolist_to_binary(lists:reverse(Acc)), Code};
        {'EXIT', Port, _Reason} ->
            {iolist_to_binary(lists:reverse(Acc)), 1}
    end.

%% Gleam hands us a list of {binary(), binary()} pairs; ports want strings.
env_pairs(Env) ->
    [{binary_to_list(K), binary_to_list(V)} || {K, V} <- Env].

read_file_or_empty(Path) ->
    case file:read_file(Path) of
        {ok, Contents} -> Contents;
        _ -> <<>>
    end.

tmp_path(Label) ->
    Dir = case os:getenv("TMPDIR") of
        false -> "/tmp";
        "" -> "/tmp";
        Value -> Value
    end,
    Unique = erlang:integer_to_list(erlang:unique_integer([positive])),
    filename:join(Dir, "bsdkrun-" ++ Unique ++ "-" ++ Label).

%% --- host lookups -----------------------------------------------------------

find_executable(Name) ->
    case os:find_executable(binary_to_list(Name)) of
        false -> {error, nil};
        Path -> {ok, list_to_binary(Path)}
    end.

get_env(Name) ->
    case os:getenv(binary_to_list(Name)) of
        false -> {error, nil};
        "" -> {error, nil};
        Value -> {ok, list_to_binary(Value)}
    end.

file_exists(Path) ->
    filelib:is_regular(binary_to_list(Path)).

%% The current working directory. Used to walk up looking for an in-repo dev
%% build of the binary — the compiled beam's own path lives under `build/`,
%% which says nothing useful once the package is published.
cwd() ->
    case file:get_cwd() of
        {ok, Dir} -> {ok, list_to_binary(Dir)};
        _ -> {error, nil}
    end.

%% --- binary override cache --------------------------------------------------

set_override(Path) ->
    persistent_term:put(?OVERRIDE_KEY, Path),
    nil.

get_override() ->
    case persistent_term:get(?OVERRIDE_KEY, undefined) of
        undefined -> {error, nil};
        Path -> {ok, Path}
    end.

clear_override() ->
    _ = persistent_term:erase(?OVERRIDE_KEY),
    nil.
