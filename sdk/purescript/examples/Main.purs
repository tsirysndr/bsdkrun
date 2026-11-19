module Main where

import Prelude
import Bsdkrun as B
import Bsdkrun.Exec as Exec
import Bsdkrun.Options as Options
import Effect (Effect)
import Effect.Aff (bracket, launchAff_)
import Effect.Class (liftEffect)
import Effect.Console (log)

main :: Effect Unit
main = launchAff_ do
  let
    options = Options.linux "alpine"
      # Options.withName "web"
      # Options.withCpus 2
      # Options.withMem 1024
      # Options.withPorts [ "8080:80" ]
      # Options.withCommand [ "sleep", "infinity" ]
  bracket (options # B.create) (void <<< B.remove true) \box -> do
    result <- box # B.exec [ "uname", "-a" ]
    liftEffect $ log $ Exec.text result
