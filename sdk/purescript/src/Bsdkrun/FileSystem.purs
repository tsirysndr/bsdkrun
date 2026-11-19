module Bsdkrun.FileSystem where

import Prelude
import Effect.Aff (Aff)
import Bsdkrun.Internal (Async, toAff)
import Bsdkrun.Types (Sandbox, Bytes)

writeText :: String -> String -> Sandbox -> Aff (Unit)
writeText path text box = toAff (writeTextImpl path text box)

foreign import writeTextImpl :: String -> String -> Sandbox -> Async (Unit)

writeFile :: String -> Bytes -> Sandbox -> Aff (Unit)
writeFile path bytes box = toAff (writeFileImpl path bytes box)

foreign import writeFileImpl :: String -> Bytes -> Sandbox -> Async (Unit)

readText :: String -> Sandbox -> Aff (String)
readText path box = toAff (readTextImpl path box)

foreign import readTextImpl :: String -> Sandbox -> Async (String)

readFile :: String -> Sandbox -> Aff (Bytes)
readFile path box = toAff (readFileImpl path box)

foreign import readFileImpl :: String -> Sandbox -> Async (Bytes)

upload :: String -> String -> Sandbox -> Aff (Unit)
upload local remote box = toAff (uploadImpl local remote box)

foreign import uploadImpl :: String -> String -> Sandbox -> Async (Unit)

download :: String -> String -> Boolean -> Sandbox -> Aff (Unit)
download remote local recursive box = toAff (downloadImpl remote local recursive box)

foreign import downloadImpl :: String -> String -> Boolean -> Sandbox -> Async (Unit)
