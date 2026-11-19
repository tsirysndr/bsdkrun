module Test.Main where

import Prelude
import Bsdkrun as B
import Bsdkrun.Cache as Cache
import Bsdkrun.CI as CI
import Bsdkrun.Client as Client
import Bsdkrun.Error as Error
import Bsdkrun.Exec as Exec
import Bsdkrun.Options as Options
import Bsdkrun.Types (Bytes)
import Data.Either (Either(..))
import Data.Maybe (Maybe(..))
import Data.Nullable (null, toMaybe)
import Effect (Effect)
import Effect.Aff (Aff, attempt, bracket, launchAff_, throwError)
import Effect.Class (liftEffect)
import Effect.Console (log)
import Effect.Exception (error)
import Foreign.Object as Object
import Bsdkrun.FileSystem as FS

foreign import setup :: Effect { args :: Effect (Array String), cleanup :: Effect Unit }
foreign import makeSink :: Effect { write :: Bytes -> Effect Unit, read :: Effect Bytes }
foreign import mockDaemon :: Effect { input :: Effect String, linuxInput :: Effect { image :: String, name :: String, network :: String, noNet :: Boolean, env :: Array String, valid :: Boolean }, cleanup :: Effect Unit }

assert :: String -> Boolean -> Aff Unit
assert message ok = unless ok $ throwError $ error message

main :: Effect Unit
main = launchAff_ do
  assert "text preserves significant spaces" (Exec.text { stdout: "  value \n\n", stderr: "", exitCode: 0 } == "  value ")
  let base = Options.linux "alpine"
  let
    opts = base # Options.withName "web" # Options.withCpus 2
      # Options.withNetwork "devnet"
      # Options.withPorts [ "8080:80" ]
      # Options.withEnv (Object.singleton "A" "$(touch unsafe); spaced")
      # Options.withCommand [ "printf", "%s", "hello world" ]
  assert "immutable options" (Options.buildCreateArgs base == [ "linux", "alpine", "-d" ])
  assert "structured create arguments"
    ( Options.buildCreateArgs opts ==
        [ "linux", "alpine", "-d", "-e", "A=$(touch unsafe); spaced", "--port", "8080:80", "--network", "devnet", "--name", "web", "--cpus", "2", "--", "printf", "%s", "hello world" ]
    )
  assert "BSD options" ((Options.freebsd # Options.withVersion "15.0" # Options.withPersist true # Options.buildCreateArgs) == [ "freebsd", "-d", "--version", "15.0", "--persist" ])
  assert "firmware" (Options.buildCreateArgs (Options.firmware "uefi" "disk") == [ "firmware", "--firmware", "uefi", "--disk", "disk", "-d" ])
  assert "kernel" (Options.buildCreateArgs (Options.kernel "vmlinuz") == [ "kernel", "--kernel", "vmlinuz", "-d" ])
  assert "NetBSD" (Options.buildCreateArgs Options.netbsd == [ "netbsd", "-d" ])
  assert "Nanos" (Options.buildCreateArgs (Options.nanos "disk") == [ "nanos", "-d", "disk" ])
  assert "Unikraft" (Options.buildCreateArgs (Options.unikraft ".") == [ "unikraft", "-d", "." ])
  assert "Solo5" (Options.buildCreateArgs (Options.solo5 ".") == [ "solo5", "-d", "." ])
  assert "OSv" (Options.buildCreateArgs (Options.osv "loader") == [ "osv", "-d", "loader" ])
  let prefix = CI.workflow "test" # CI.onPush [ "main" ] # CI.deps [ "nodejs" ]
  let plan = prefix # CI.step "test" "npm test"
  assert "workflow immutable prefix" (CI.yaml prefix /= CI.yaml plan)
  bracket (liftEffect setup) (liftEffect <<< _.cleanup) \fake -> do
    box <- opts # B.create
    chained <- pure box # B.andThen B.stop # B.andThen (B.connectNetwork "devnet") # B.andThen B.start
    assert "lifecycle preserves sandbox" (B.sandboxId box == B.sandboxId chained)
    out <- liftEffect makeSink
    err <- liftEffect makeSink
    let
      execOpts = Exec.options # Exec.withStdin "input"
        # Exec.withStdout out.write
        # Exec.withStderr err.write
    result <- box # B.execWith
      [ "node", "-e", "process.stdin.resume();process.stdin.on('data',d=>process.stdout.write(d));process.stdout.write('é'.repeat(80000));process.stderr.write('err'.repeat(80000));process.exitCode=7" ]
      execOpts
    liveOut <- liftEffect out.read
    liveErr <- liftEffect err.read
    assert "nonzero exit remains result data" (result.exitCode == 7)
    assert "live stdout retained" (Exec.decodeUtf8 liveOut == result.stdout)
    assert "live stderr retained" (Exec.decodeUtf8 liveErr == result.stderr)
    args <- liftEffect fake.args
    assert "streaming does not request a PTY"
      ( args ==
          [ "exec", "abcdef123456", "node", "-e", "process.stdin.resume();process.stdin.on('data',d=>process.stdout.write(d));process.stdout.write('é'.repeat(80000));process.stderr.write('err'.repeat(80000));process.exitCode=7" ]
      )
    failed <- attempt $ box # B.execWith [ "node", "-e", "process.stdout.write('kept');process.exitCode=9" ] (Exec.options # Exec.withThrowOnError true)
    case failed of
      Left e -> case Error.commandResult e of
        Just r -> assert "throw mode retains output" (r.exitCode == 9 && r.stdout == "kept")
        Nothing -> assert "expected command error" false
      Right _ -> assert "throw mode" false
    literal <- box # B.exec [ "node", "-e", "process.stdout.write(process.argv[1])", "$(echo injected); spaced" ]
    assert "argv stays literal" (literal.stdout == "$(echo injected); spaced")
    let bytes = Exec.fromBytes [ 0, 255, 254, 10, 128 ]
    box # FS.writeFile "/tmp/data" bytes
    roundtrip <- box # FS.readFile "/tmp/data"
    assert "byte-exact files" (Exec.toBytes roundtrip == Exec.toBytes bytes)
    hit <- box # Cache.restore { key: "miss", path: null, restoreKeys: [] }
    assert "cache miss succeeds with nullable fields" (not hit.restored && toMaybe hit.key == Nothing)
  bracket (liftEffect mockDaemon) (liftEffect <<< _.cleanup) \daemon -> do
    client <- Client.connect "http://example.test" "secret"
    _ <- client # Client.list false
    variables <- liftEffect daemon.input
    assert "remote list boolean" (variables == "{\"all\":false}")
    id <- client # Client.runLinux (Options.linux "alpine" # Options.withName "remote" # Options.withNetwork "devnet")
    assert "remote create" (id == "remote-id")
    input <- liftEffect daemon.linuxInput
    assert "remote network translated without CLI-only fields" (input.image == "alpine" && input.name == "remote" && input.network == "devnet" && not input.noNet && input.valid && input.env == [])

  liftEffect $ log "PureScript SDK tests passed"
