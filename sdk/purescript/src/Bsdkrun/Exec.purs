module Bsdkrun.Exec where

import Prelude
import Bsdkrun.Types (Bytes, CommandResult, ExecOptions)
import Effect (Effect)
import Foreign.Object (Object)
import Data.Maybe (Maybe(..))
import Data.String (Pattern(..), stripSuffix)

foreign import options :: ExecOptions
foreign import withEnv :: Object String -> ExecOptions -> ExecOptions
foreign import withStdin :: String -> ExecOptions -> ExecOptions
foreign import withInput :: Bytes -> ExecOptions -> ExecOptions
foreign import withCwd :: String -> ExecOptions -> ExecOptions
foreign import withTty :: Boolean -> ExecOptions -> ExecOptions
foreign import withThrowOnError :: Boolean -> ExecOptions -> ExecOptions
foreign import withLogLevel :: Int -> ExecOptions -> ExecOptions
foreign import withStdout :: (Bytes -> Effect Unit) -> ExecOptions -> ExecOptions
foreign import withStderr :: (Bytes -> Effect Unit) -> ExecOptions -> ExecOptions
foreign import encodeUtf8 :: String -> Bytes
foreign import decodeUtf8 :: Bytes -> String
foreign import fromBytes :: Array Int -> Bytes
foreign import toBytes :: Bytes -> Array Int

text :: CommandResult -> String
text result = trimNewlines result.stdout
  where
  trimNewlines value = case stripSuffix (Pattern "\n") value of
    Just rest -> trimNewlines rest
    Nothing -> value

isOk :: CommandResult -> Boolean
isOk result = result.exitCode == 0
