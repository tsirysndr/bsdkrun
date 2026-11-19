-- | Node.js PureScript SDK. Configuration and sandbox arguments come last
-- | so functions compose with Prelude's (#) and Aff actions with (# bind ...).
module Bsdkrun (module Sandbox, module Types, andThen) where

import Prelude
import Effect.Aff (Aff)

import Bsdkrun.Sandbox (create, get, list, exec, execWith, sandboxId, status, isRunning, logs, stop, start, remove, update, connectNetwork, disconnectNetwork, setBinaryPath, resolveBinary) as Sandbox
import Bsdkrun.Types (Sandbox, SandboxInfo, CreateOptions, ExecOptions, CommandResult, Bytes) as Types

-- | Flipped bind for left-to-right (#) chains.
andThen :: forall a b. (a -> Aff b) -> Aff a -> Aff b
andThen = flip bind
