module Bsdkrun.Client where

import Data.Nullable (Nullable)

import Effect.Aff (Aff)
import Bsdkrun.Internal (Async, toAff)
import Bsdkrun.Types (Client, SandboxInfo, CommandResult, CreateOptions, RemoteResult)
import Data.Argonaut.Core (Json)
import Foreign.Object (Object)

connect :: String -> String -> Aff (Client)
connect url token = toAff (connectImpl url token)

foreign import connectImpl :: String -> String -> Async (Client)

fromEnv :: Aff (Client)
fromEnv = toAff (fromEnvImpl)

foreign import fromEnvImpl :: Async (Client)

request :: String -> Object Json -> Client -> Aff (Json)
request query variables client = toAff (requestImpl query variables client)

foreign import requestImpl :: String -> Object Json -> Client -> Async (Json)

list :: Boolean -> Client -> Aff (Array SandboxInfo)
list all client = toAff (listImpl all client)

foreign import listImpl :: Boolean -> Client -> Async (Array SandboxInfo)

runLinux :: CreateOptions -> Client -> Aff (String)
runLinux opts client = toAff (runLinuxImpl opts client)

foreign import runLinuxImpl :: CreateOptions -> Client -> Async (String)

exec :: String -> Array String -> Array String -> Client -> Aff (RemoteResult)
exec id command env client = toAff (execImpl id command env client)

foreign import execImpl :: String -> Array String -> Array String -> Client -> Async (RemoteResult)

stop :: String -> Client -> Aff (CommandResult)
stop id client = toAff (stopImpl id client)

foreign import stopImpl :: String -> Client -> Async (CommandResult)

start :: String -> Client -> Aff (CommandResult)
start id client = toAff (startImpl id client)

foreign import startImpl :: String -> Client -> Async (CommandResult)

remove :: Array String -> Boolean -> Client -> Aff (CommandResult)
remove ids force client = toAff (removeImpl ids force client)

foreign import removeImpl :: Array String -> Boolean -> Client -> Async (CommandResult)

logs :: String -> Boolean -> Client -> Aff (String)
logs id boot client = toAff (logsImpl id boot client)

foreign import logsImpl :: String -> Boolean -> Client -> Async (String)

get :: String -> Client -> Aff (Nullable SandboxInfo)
get id client = toAff (getImpl id client)

foreign import getImpl :: String -> Client -> Async (Nullable SandboxInfo)

update :: String -> Int -> Int -> Client -> Aff (CommandResult)
update id cpus mem client = toAff (updateImpl id cpus mem client)

foreign import updateImpl :: String -> Int -> Int -> Client -> Async (CommandResult)

commit :: String -> String -> String -> Client -> Aff (CommandResult)
commit id name description client = toAff (commitImpl id name description client)

foreign import commitImpl :: String -> String -> String -> Client -> Async (CommandResult)

runBsd :: CreateOptions -> Client -> Aff (String)
runBsd opts client = toAff (runBsdImpl opts client)

foreign import runBsdImpl :: CreateOptions -> Client -> Async (String)

run :: CreateOptions -> Client -> Aff String
run opts client = toAff (runImpl opts client)

foreign import runImpl :: CreateOptions -> Client -> Async String
