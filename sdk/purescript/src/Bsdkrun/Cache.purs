module Bsdkrun.Cache where

import Prelude
import Effect.Aff (Aff)
import Bsdkrun.Internal (Async, toAff)
import Bsdkrun.Types (Sandbox, CacheEntry, RestoreResult)
import Data.Nullable (Nullable)

save :: String -> { key :: String, compression :: String, force :: Boolean } -> Sandbox -> Aff (CacheEntry)
save path opts box = toAff (saveImpl path opts box)

foreign import saveImpl :: String -> { key :: String, compression :: String, force :: Boolean } -> Sandbox -> Async (CacheEntry)

restore :: { key :: String, path :: Nullable String, restoreKeys :: Array String } -> Sandbox -> Aff (RestoreResult)
restore opts box = toAff (restoreImpl opts box)

foreign import restoreImpl :: { key :: String, path :: Nullable String, restoreKeys :: Array String } -> Sandbox -> Async (RestoreResult)

list :: Aff (Array CacheEntry)
list = toAff (listImpl)

foreign import listImpl :: Async (Array CacheEntry)

remove :: Array String -> Boolean -> Aff (Unit)
remove keys all = toAff (removeImpl keys all)

foreign import removeImpl :: Array String -> Boolean -> Async (Unit)
