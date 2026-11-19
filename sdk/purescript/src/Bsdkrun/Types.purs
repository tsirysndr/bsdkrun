module Bsdkrun.Types where

import Data.Nullable (Nullable)

foreign import data Sandbox :: Type
foreign import data Client :: Type
-- | Byte-exact Uint8Array, including Node Buffers. Never decoded implicitly.
foreign import data Bytes :: Type
foreign import data CreateOptions :: Type
foreign import data ExecOptions :: Type

type CommandResult = { stdout :: String, stderr :: String, exitCode :: Int }
type RemoteResult = { output :: Bytes, exitCode :: Int }
type PortForward = { bind :: String, host :: Int, guest :: Int }
type SandboxInfo =
  { id :: String
  , name :: Nullable String
  , image :: String
  , kind :: String
  , command :: String
  , status :: String
  , running :: Boolean
  , exitCode :: Nullable Int
  , pid :: Nullable Int
  , detached :: Boolean
  , cpus :: Int
  , mem :: Int
  , volume :: Nullable String
  , stateDir :: String
  , network :: Nullable String
  , netIp :: Nullable String
  , ports :: Array PortForward
  , createdAt :: Number
  , finishedAt :: Nullable Number
  }

type ImageInfo = { id :: String, reference :: String, digest :: String, size :: Number, rootfs :: String, createdAt :: Number }
type VolumeInfo = { name :: String, guest :: Nullable String, base :: Nullable String, path :: String, size :: String, createdAt :: Nullable Number, tracked :: Boolean }
type NetworkInfo = { name :: String, subnet :: String, gateway :: String, members :: Int, running :: Int, up :: Boolean, createdAt :: Nullable Number }
type CacheEntry = { key :: String, path :: String, compression :: String, size :: Number, created :: Number, digest :: String }
type RestoreResult = { restored :: Boolean, requestedKey :: String, key :: Nullable String, path :: Nullable String, size :: Nullable Number, compression :: Nullable String, created :: Nullable Number }
