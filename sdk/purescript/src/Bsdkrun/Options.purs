module Bsdkrun.Options where

import Foreign.Object (Object)
import Bsdkrun.Types (CreateOptions)

foreign import linux :: String -> CreateOptions
foreign import freebsd :: CreateOptions
foreign import netbsd :: CreateOptions
foreign import firmware :: String -> String -> CreateOptions
foreign import kernel :: String -> CreateOptions
foreign import nanos :: String -> CreateOptions
foreign import unikraft :: String -> CreateOptions
foreign import solo5 :: String -> CreateOptions
foreign import osv :: String -> CreateOptions
foreign import withName :: String -> CreateOptions -> CreateOptions
foreign import withCpus :: Int -> CreateOptions -> CreateOptions
foreign import withMem :: Int -> CreateOptions -> CreateOptions
foreign import withLogLevel :: Int -> CreateOptions -> CreateOptions
foreign import withVolume :: String -> CreateOptions -> CreateOptions
foreign import withPersist :: Boolean -> CreateOptions -> CreateOptions
foreign import withVersion :: String -> CreateOptions -> CreateOptions
foreign import withMounts :: Array String -> CreateOptions -> CreateOptions
foreign import withDisks :: Array String -> CreateOptions -> CreateOptions
foreign import withCommand :: Array String -> CreateOptions -> CreateOptions
foreign import withEnv :: Object String -> CreateOptions -> CreateOptions
foreign import withKernel :: String -> CreateOptions -> CreateOptions
foreign import withKernelVersion :: String -> CreateOptions -> CreateOptions
foreign import withCmdline :: String -> CreateOptions -> CreateOptions
foreign import withEntrypoint :: String -> CreateOptions -> CreateOptions
foreign import withBlocks :: Array String -> CreateOptions -> CreateOptions
foreign import withArgs :: Array String -> CreateOptions -> CreateOptions
foreign import withDisk :: String -> CreateOptions -> CreateOptions
foreign import withFirmware :: String -> CreateOptions -> CreateOptions
foreign import withForce :: Boolean -> CreateOptions -> CreateOptions
foreign import withFormat :: String -> CreateOptions -> CreateOptions
foreign import withInitramfs :: String -> CreateOptions -> CreateOptions
foreign import withLinuxInitramfs :: Boolean -> CreateOptions -> CreateOptions
foreign import withGic :: String -> CreateOptions -> CreateOptions
foreign import withConsole :: String -> CreateOptions -> CreateOptions
foreign import withNetwork :: String -> CreateOptions -> CreateOptions
foreign import withPorts :: Array String -> CreateOptions -> CreateOptions
foreign import withMac :: String -> CreateOptions -> CreateOptions
foreign import withNoNet :: Boolean -> CreateOptions -> CreateOptions
foreign import buildCreateArgs :: CreateOptions -> Array String
