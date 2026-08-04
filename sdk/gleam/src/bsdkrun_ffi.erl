%% Erlang FFI for the bsdkrun Gleam SDK.
%%
%% Everything Gleam cannot express on its own: spawning the `bsdkrun` binary
%% and capturing its streams, PATH/env lookups, and the cached binary override.
-module(bsdkrun_ffi).

-export([
    run/4,
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
run(Bin, Args, Env, Stdin) ->
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
            port_run("/bin/sh", ["-c", Script, "sh", Bin | Args], PortEnv),
        {output, Stdout, read_file_or_empty(StderrPath), Code}
    after
        _ = file:delete(StderrPath),
        [file:delete(P) || P <- Cleanup]
    end.

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
