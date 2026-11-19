module Bsdkrun.Host where

import Prelude
import Effect.Aff (Aff)
import Bsdkrun.Internal (Async, toAff)
import Bsdkrun.Types (ImageInfo, VolumeInfo, NetworkInfo, SandboxInfo)

images :: Aff (Array ImageInfo)
images = toAff (imagesImpl)

foreign import imagesImpl :: Async (Array ImageInfo)

volumes :: Aff (Array VolumeInfo)
volumes = toAff (volumesImpl)

foreign import volumesImpl :: Async (Array VolumeInfo)

removeVolumes :: Array String -> Boolean -> Aff (Unit)
removeVolumes names force = toAff (removeVolumesImpl names force)

foreign import removeVolumesImpl :: Array String -> Boolean -> Async (Unit)

networks :: Aff (Array NetworkInfo)
networks = toAff (networksImpl)

foreign import networksImpl :: Async (Array NetworkInfo)

createNetwork :: String -> Aff (Unit)
createNetwork name = toAff (createNetworkImpl name)

foreign import createNetworkImpl :: String -> Async (Unit)

removeNetworks :: Array String -> Boolean -> Aff (Unit)
removeNetworks names force = toAff (removeNetworksImpl names force)

foreign import removeNetworksImpl :: Array String -> Boolean -> Async (Unit)

syncNetwork :: String -> Aff (Unit)
syncNetwork name = toAff (syncNetworkImpl name)

foreign import syncNetworkImpl :: String -> Async (Unit)

networkMembers :: String -> Aff (Array SandboxInfo)
networkMembers name = toAff (networkMembersImpl name)

foreign import networkMembersImpl :: String -> Async (Array SandboxInfo)

probe :: Aff (Boolean)
probe = toAff (probeImpl)

foreign import probeImpl :: Async (Boolean)

versions :: String -> Aff (String)
versions os = toAff (versionsImpl os)

foreign import versionsImpl :: String -> Async (String)

fetchImage :: String -> { version :: String, dir :: String, force :: Boolean } -> Aff (String)
fetchImage os opts = toAff (fetchImageImpl os opts)

foreign import fetchImageImpl :: String -> { version :: String, dir :: String, force :: Boolean } -> Async (String)

growDisk :: String -> String -> Aff (Unit)
growDisk disk size = toAff (growDiskImpl disk size)

foreign import growDiskImpl :: String -> String -> Async (Unit)
