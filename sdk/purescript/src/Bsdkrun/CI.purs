module Bsdkrun.CI where

import Prelude
import Effect (Effect)
import Effect.Aff (Aff)
import Bsdkrun.Internal (Async, toAff)
import Foreign.Object (Object)

foreign import data Workflow :: Type
foreign import workflow :: String -> Workflow
foreign import engine :: String -> Workflow -> Workflow
foreign import onPush :: Array String -> Workflow -> Workflow
foreign import onPullRequest :: Array String -> Workflow -> Workflow
foreign import deps :: Array String -> Workflow -> Workflow
foreign import depsFrom :: String -> Array String -> Workflow -> Workflow
foreign import env :: String -> String -> Workflow -> Workflow
foreign import step :: String -> String -> Workflow -> Workflow
foreign import stepWithEnv :: String -> String -> Object String -> Workflow -> Workflow
foreign import cloneDepth :: Int -> Workflow -> Workflow
foreign import skipClone :: Workflow -> Workflow
foreign import yaml :: Workflow -> String
foreign import save :: String -> Workflow -> Effect String
foreign import runImpl :: String -> Workflow -> Async Unit

run :: String -> Workflow -> Aff Unit
run dir plan = toAff (runImpl dir plan)
